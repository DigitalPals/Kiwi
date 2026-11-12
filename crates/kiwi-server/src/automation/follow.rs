//! Tweak updates: Kiwi looks on GitHub for newer sources of the iOS apps' tweaks and builds
//! them by itself. A package the recipe lets follow (`"follow"`) moves to its repository's
//! newest release that names the versions it supports, or to its branch's newest commit
//! that is a day old (a series pushed in one go settles first). Sources only move forward:
//! a newer commit must descend from the one in use (GitHub's compare), so rewritten history
//! is ignored.
//!
//! Newer sources are a candidate until a build with them succeeds; every build tries the
//! candidate, and after [`ATTEMPTS`] failed builds Kiwi gives it up and keeps building with
//! the sources that last built, until upstream moves on. A Kiwi update that moves a recipe's
//! own pin drops what was found against the old one.
use std::{
    collections::BTreeMap,
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use chrono::Utc;
use kiwi_source::Url;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};

use super::{Package, Recipe, recipes};
use crate::{Kiwi, db::Time};

pub const GITHUB_API: &str = "https://api.github.com";
/// How often Kiwi looks upstream: about 15 GitHub requests for all apps, well within the
/// 60 an hour GitHub allows without an account.
const EVERY: Duration = Duration::from_secs(3 * 3600);
/// A branch's commits are used once they are this old.
pub const SETTLE_HOURS: i64 = 24;
/// Failed builds with newer sources before Kiwi gives them up.
pub const ATTEMPTS: u32 = 2;
const MAX_JSON: u64 = 2 * 1024 * 1024;

/// How a recipe package follows its repository.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Spec {
    /// The branch whose settled commits it follows.
    #[serde(default)]
    pub branch: Option<String>,
    /// The releases it follows.
    #[serde(default)]
    pub release: Option<ReleaseSpec>,
}

/// Releases to follow. Templates match `{version}` (the tweak's) and `{app_version}` (the
/// App Store version it supports) to dotted version numbers.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ReleaseSpec {
    /// The release tag, e.g. `v{app_version}-{version}`.
    pub tag: String,
    /// Only versions starting with this (a major version line).
    #[serde(default)]
    pub version_prefix: Option<String>,
    /// A release file whose name says the app version, when the tag does not.
    #[serde(default)]
    pub asset: Option<String>,
}

impl Spec {
    /// Whether this follows the app version a release names (the recipe pins one).
    pub fn names_app_version(&self) -> bool {
        self.release.as_ref().is_some_and(|r| {
            r.tag.contains("{app_version}")
                || r.asset
                    .as_ref()
                    .is_some_and(|a| a.contains("{app_version}"))
        })
    }
}

/// A source newer than the recipe's.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Pin {
    /// The commit.
    pub sha: String,
    /// The recipe's commit it was found against.
    pub base: String,
    /// The release, for a package that follows releases.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tag: Option<String>,
    /// The tweak's version, from the release.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// The App Store version the release supports.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub app_version: Option<String>,
    /// When the commit was made.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub date: Option<Time>,
    pub found_at: Time,
}

/// Newer sources by package id; packages absent use the recipe's.
pub type Pins = BTreeMap<String, Pin>;

/// Newer sources that failed to build.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Failed {
    pub pins: Pins,
    pub error: String,
    pub at: Time,
}

/// One app's tweak updates (in its `AppState`).
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Follow {
    /// The sources builds use: the last that built.
    pub current: Pins,
    /// Newer sources waiting for a build that succeeds; builds try them first.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub candidate: Option<Pins>,
    /// Whether a build has tried `candidate` yet.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub tried: bool,
    /// Failed builds with `candidate`.
    #[serde(skip_serializing_if = "is_zero")]
    pub attempts: u32,
    /// Sources Kiwi gave up on; tried again only once upstream moves on.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed: Option<Failed>,
    /// When Kiwi last looked upstream.
    pub checked_at: Option<Time>,
    /// Why that look didn't finish (GitHub unreachable or limiting requests).
    pub error: Option<String>,
}

fn is_zero(n: &u32) -> bool {
    *n == 0
}

/// Whether two sets name the same commits.
fn same(a: &Pins, b: &Pins) -> bool {
    a.len() == b.len()
        && a.iter()
            .all(|(id, p)| b.get(id).is_some_and(|q| q.sha == p.sha))
}

/// The packages of iOS recipes that follow upstream.
fn followed(recipe: &Recipe) -> impl Iterator<Item = (&Package, &Spec, &str, &str)> {
    recipe.packages.iter().filter_map(|p| {
        let spec = p.follow.as_ref()?;
        let (repo, sha) = p.repo.as_ref()?;
        Some((p, spec, repo.as_str(), sha.as_str()))
    })
}

/// Whether Kiwi follows any of the recipe's tweaks.
pub fn follows(recipe: &Recipe) -> bool {
    !recipe.android() && followed(recipe).next().is_some()
}

// --- builds -----------------------------------------------------------------------------

/// The sources a build uses, and whether they are the candidate: none where Kiwi doesn't
/// follow tweak updates.
pub fn for_build(follow: &Follow, on: bool) -> (Pins, bool) {
    match (&follow.candidate, on) {
        (_, false) => (Pins::new(), false),
        (Some(candidate), true) => (candidate.clone(), true),
        (None, true) => (follow.current.clone(), false),
    }
}

/// Whether the app has a candidate no build has tried yet: it builds before its next check.
pub fn pending(follow: &Follow, on: bool) -> bool {
    on && follow.candidate.is_some() && !follow.tried
}

/// The runner's `pins` for a build (`apply_pins` in runner.py), `None` without newer sources.
pub fn request(recipe: &Recipe, pins: &Pins) -> Option<Value> {
    if pins.is_empty() {
        return None;
    }
    let mut out = Map::new();
    out.insert(
        "packages".into(),
        pins.iter()
            .map(|(id, p)| (id.clone(), Value::String(p.sha.clone())))
            .collect::<Map<_, _>>()
            .into(),
    );
    if recipe.app_version.is_some()
        && let Some(version) = app_version(pins)
    {
        out.insert("app_version".into(), version.into());
    }
    if let Some(tweak) = tweak_name(recipe, pins) {
        out.insert("tweak".into(), tweak.into());
    }
    Some(out.into())
}

fn app_version(pins: &Pins) -> Option<String> {
    pins.values().find_map(|p| p.app_version.clone())
}

/// The tweak's name with its newer version: the recipe's first package names the tweak.
/// A package following a branch has no version to show, so the name stands alone.
pub fn tweak_name(recipe: &Recipe, pins: &Pins) -> Option<String> {
    let pin = pins.get(&recipe.packages.first()?.id)?;
    let name = recipe.tweak.split_whitespace().next()?;
    Some(match &pin.version {
        Some(version) => format!("{name} {version}"),
        None => name.to_owned(),
    })
}

/// The tweak and supported App Store version the app builds with now.
pub fn shown(recipe: &Recipe, follow: &Follow, on: bool) -> (String, Option<String>) {
    if !on || follow.current.is_empty() {
        return (recipe.tweak.clone(), recipe.app_version.clone());
    }
    let app = recipe
        .app_version
        .as_ref()
        .and(app_version(&follow.current))
        .or_else(|| recipe.app_version.clone());
    (
        tweak_name(recipe, &follow.current).unwrap_or_else(|| recipe.tweak.clone()),
        app,
    )
}

/// After a build with the candidate: keep the sources when it built, give them up after
/// [`ATTEMPTS`] failures. Cancelled builds and a lost Mac builder don't count.
pub fn finish(follow: &mut Follow, pins: &Pins, status: &str, error: Option<&str>, counts: bool) {
    let is_candidate = follow.candidate.as_ref().is_some_and(|c| same(c, pins));
    match status {
        "ready" | "unchanged" => {
            follow.current = pins.clone();
            if is_candidate {
                follow.candidate = None;
                follow.tried = false;
                follow.attempts = 0;
            }
        }
        "failed" | "blocked" if counts && is_candidate => {
            follow.attempts += 1;
            if follow.attempts >= ATTEMPTS {
                follow.failed = Some(Failed {
                    pins: pins.clone(),
                    error: error.unwrap_or("The build failed").to_owned(),
                    at: Utc::now(),
                });
                follow.candidate = None;
                follow.tried = false;
                follow.attempts = 0;
            }
        }
        _ => {}
    }
}

/// The newest sources found upstream (`current` with what moved).
pub fn offer(follow: &mut Follow, newest: Pins) {
    if same(&newest, &follow.current) {
        follow.candidate = None;
        follow.tried = false;
        follow.attempts = 0;
        return;
    }
    let known = |p: Option<&Pins>| p.is_some_and(|p| same(p, &newest));
    if known(follow.candidate.as_ref()) || known(follow.failed.as_ref().map(|f| &f.pins)) {
        return;
    }
    follow.candidate = Some(newest);
    follow.tried = false;
    follow.attempts = 0;
    follow.failed = None;
}

/// Drops sources found against a recipe pin this Kiwi no longer has (a Kiwi update moved
/// it) or for packages that no longer follow.
pub fn prune(recipe: &Recipe, follow: &mut Follow) {
    let valid = |id: &str, pin: &Pin| {
        followed(recipe).any(|(p, _, _, sha)| p.id == id && pin.base == sha && pin.sha != sha)
    };
    let complete = |pins: &Pins| pins.iter().all(|(id, p)| valid(id, p));
    follow.current.retain(|id, p| valid(id, p));
    if follow.candidate.as_ref().is_some_and(|c| !complete(c)) {
        follow.candidate = None;
        follow.tried = false;
        follow.attempts = 0;
    }
    if follow.failed.as_ref().is_some_and(|f| !complete(&f.pins)) {
        follow.failed = None;
    }
}

// --- looking upstream -------------------------------------------------------------------

/// Looks upstream in the background now and then, while Kiwi Builds and tweak updates are on.
pub struct Follower {
    /// Injectable for tests.
    pub github_api: RwLock<String>,
    /// Whether the scheduler starts rounds (tests call [`check`] themselves).
    pub background: AtomicBool,
    busy: AtomicBool,
    last: Mutex<Option<Instant>>,
}

impl Default for Follower {
    fn default() -> Self {
        Self {
            github_api: RwLock::new(GITHUB_API.into()),
            background: AtomicBool::new(true),
            busy: AtomicBool::new(false),
            last: Mutex::new(None),
        }
    }
}

/// Starts a round when one is due.
pub fn kick(kiwi: &Arc<Kiwi>) {
    let f = &kiwi.automation.follow;
    if super::require_on(kiwi).is_err()
        || !kiwi.settings().follow_tweak_updates
        || !f.background.load(Ordering::Relaxed)
    {
        return;
    }
    {
        let mut last = f.last.lock().unwrap();
        if last.is_some_and(|t| t.elapsed() < EVERY) || f.busy.swap(true, Ordering::SeqCst) {
            return;
        }
        *last = Some(Instant::now());
    }
    let kiwi = kiwi.clone();
    tokio::spawn(async move {
        check(&kiwi).await;
        kiwi.automation.follow.busy.store(false, Ordering::SeqCst);
    });
}

/// Looks for newer sources of every added iOS app Kiwi can build, and offers them.
pub async fn check(kiwi: &Arc<Kiwi>) {
    let api = kiwi.automation.follow.github_api.read().unwrap().clone();
    let state = kiwi.automation.snapshot();
    for recipe in recipes().iter().filter(|r| follows(r)) {
        let Some(app) = state.apps.get(&recipe.id) else {
            continue;
        };
        if !app.added || !kiwi.automation.can_build(recipe) {
            continue;
        }
        let result = newest(kiwi, &api, recipe, &app.follow).await;
        if let Err(e) = &result {
            tracing::warn!(app = %recipe.id, "looking for tweak updates failed: {e}");
        }
        let changed = kiwi.automation.app(&kiwi.db, &recipe.id, |a| {
            let before = a.follow.candidate.clone();
            a.follow.checked_at = Some(Utc::now());
            match result {
                // A build changed what builds use meanwhile: the next round looks again.
                Ok(_) if !same(&a.follow.current, &app.follow.current) => {}
                Ok(pins) => {
                    a.follow.error = None;
                    offer(&mut a.follow, pins);
                }
                Err(e) => a.follow.error = Some(e),
            }
            Ok(a.follow.candidate != before)
        });
        if changed.unwrap_or(false) {
            tracing::info!(app = %recipe.id, "found newer tweak sources");
        }
    }
    kiwi.kick.notify_one();
}

/// `current` with every package moved to its newest source upstream that descends from it.
async fn newest(kiwi: &Kiwi, api: &str, recipe: &Recipe, follow: &Follow) -> Result<Pins, String> {
    let mut pins = follow.current.clone();
    let known = |id: &str, sha: &str| {
        [
            follow.candidate.as_ref(),
            follow.failed.as_ref().map(|f| &f.pins),
        ]
        .into_iter()
        .flatten()
        .filter_map(|p| p.get(id))
        .any(|p| p.sha == sha)
    };
    for (package, spec, repo, base) in followed(recipe) {
        let in_use = follow
            .current
            .get(&package.id)
            .map_or(base, |p| p.sha.as_str());
        let Some(found) = latest(kiwi, api, repo, spec, follow, &package.id).await? else {
            continue;
        };
        if found.sha == in_use {
            continue;
        }
        // A commit found before was compared then.
        if !known(&package.id, &found.sha) && !descends(kiwi, api, repo, in_use, &found.sha).await?
        {
            continue;
        }
        pins.insert(
            package.id.clone(),
            Pin {
                base: base.to_owned(),
                found_at: Utc::now(),
                ..found
            },
        );
    }
    Ok(pins)
}

async fn get(kiwi: &Kiwi, url: &str) -> Result<Value, String> {
    let url = Url::parse(url).map_err(|e| e.to_string())?;
    let bytes = kiwi
        .sources
        .json_bytes("GitHub", &url, MAX_JSON, &kiwi.cancel)
        .await
        .map_err(|e| format!("GitHub didn't answer: {e}"))?;
    serde_json::from_slice(&bytes).map_err(|_| "GitHub sent an unreadable answer".into())
}

/// The newest source upstream: `None` when nothing matches.
async fn latest(
    kiwi: &Kiwi,
    api: &str,
    repo: &str,
    spec: &Spec,
    follow: &Follow,
    id: &str,
) -> Result<Option<Pin>, String> {
    let api = api.trim_end_matches('/');
    if let Some(branch) = &spec.branch {
        let until =
            (Utc::now() - chrono::Duration::hours(SETTLE_HOURS)).format("%Y-%m-%dT%H:%M:%SZ");
        let commits = get(
            kiwi,
            &format!("{api}/repos/{repo}/commits?sha={branch}&until={until}&per_page=1"),
        )
        .await?;
        return Ok(commit(&commits[0]).map(|(sha, date)| Pin {
            sha,
            date,
            ..blank()
        }));
    }
    let Some(release) = &spec.release else {
        return Ok(None);
    };
    let releases = get(kiwi, &format!("{api}/repos/{repo}/releases?per_page=20")).await?;
    let Some(found) = pick_release(release, &releases) else {
        return Ok(None);
    };
    // A release seen before keeps its commit; others are looked up once.
    let seen = [
        Some(&follow.current),
        follow.candidate.as_ref(),
        follow.failed.as_ref().map(|f| &f.pins),
    ]
    .into_iter()
    .flatten()
    .filter_map(|p| p.get(id))
    .find(|p| p.tag == found.tag)
    .map(|p| (p.sha.clone(), p.date));
    let (sha, date) = match seen {
        Some(known) => known,
        None => {
            let tag = found.tag.as_deref().unwrap_or_default();
            commit(&get(kiwi, &format!("{api}/repos/{repo}/commits/{tag}")).await?)
                .ok_or_else(|| format!("GitHub has no commit for {repo} {tag}"))?
        }
    };
    Ok(Some(Pin { sha, date, ..found }))
}

fn blank() -> Pin {
    Pin {
        sha: String::new(),
        base: String::new(),
        tag: None,
        version: None,
        app_version: None,
        date: None,
        found_at: Utc::now(),
    }
}

/// A GitHub commit's hash and date.
fn commit(value: &Value) -> Option<(String, Option<Time>)> {
    let sha = value["sha"]
        .as_str()
        .filter(|s| s.len() == 40 && s.bytes().all(|b| b.is_ascii_hexdigit()))?
        .to_ascii_lowercase();
    let date = value["commit"]["committer"]["date"]
        .as_str()
        .and_then(|d| chrono::DateTime::parse_from_rfc3339(d).ok())
        .map(|d| d.with_timezone(&Utc));
    Some((sha, date))
}

/// Whether `head` is newer than `base` on the same history.
async fn descends(
    kiwi: &Kiwi,
    api: &str,
    repo: &str,
    base: &str,
    head: &str,
) -> Result<bool, String> {
    let api = api.trim_end_matches('/');
    let compare = get(kiwi, &format!("{api}/repos/{repo}/compare/{base}...{head}")).await?;
    Ok(compare["status"] == "ahead")
}

/// The newest published release whose tag (and file, where the spec names one) match.
pub fn pick_release(spec: &ReleaseSpec, releases: &Value) -> Option<Pin> {
    let needs_app = spec.tag.contains("{app_version}")
        || spec
            .asset
            .as_ref()
            .is_some_and(|a| a.contains("{app_version}"));
    for release in releases.as_array()? {
        if release["draft"] == json!(true) || release["prerelease"] == json!(true) {
            continue;
        }
        let Some(tag) = release["tag_name"].as_str() else {
            continue;
        };
        let Some(mut found) = capture(&spec.tag, tag) else {
            continue;
        };
        let version = found.remove("version");
        if spec
            .version_prefix
            .as_ref()
            .is_some_and(|prefix| !version.as_ref().is_some_and(|v| v.starts_with(prefix)))
        {
            continue;
        }
        let mut app = found.remove("app_version");
        if let Some(asset) = &spec.asset {
            app = app.or_else(|| {
                release["assets"]
                    .as_array()?
                    .iter()
                    .filter_map(|a| capture(asset, a["name"].as_str()?))
                    .find(|c| c.get("version").is_none_or(|v| Some(v) == version.as_ref()))
                    .and_then(|mut c| c.remove("app_version"))
            });
        }
        if needs_app && app.is_none() {
            continue;
        }
        return Some(Pin {
            tag: Some(tag.to_owned()),
            version,
            app_version: app,
            ..blank()
        });
    }
    None
}

/// A dotted version number: `2026.45.0`.
fn is_version(text: &str) -> bool {
    text.len() <= 32
        && text.split('.').count() <= 4
        && text
            .split('.')
            .all(|part| !part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
}

/// Matches `text` against a template whose `{name}` parts are version numbers.
pub fn capture(template: &str, text: &str) -> Option<BTreeMap<String, String>> {
    let mut out = BTreeMap::new();
    matches(template, text, &mut out).then_some(out)
}

fn matches(template: &str, text: &str, out: &mut BTreeMap<String, String>) -> bool {
    let Some(open) = template.find('{') else {
        return template == text;
    };
    let Some(close) = template[open..].find('}').map(|i| open + i) else {
        return false;
    };
    let (literal, name, rest) = (
        &template[..open],
        &template[open + 1..close],
        &template[close + 1..],
    );
    let Some(text) = text.strip_prefix(literal) else {
        return false;
    };
    // Longest version first; shorter ones when the rest needs part of it (".ipa").
    let digits = text
        .bytes()
        .take_while(|b| b.is_ascii_digit() || *b == b'.')
        .count();
    for end in (1..=digits).rev() {
        let value = &text[..end];
        if is_version(value) && matches(rest, &text[end..], out) {
            out.insert(name.to_owned(), value.to_owned());
            return true;
        }
    }
    false
}

/// What the app page shows of one followed package.
#[derive(Serialize)]
pub struct SourceView {
    id: String,
    name: String,
    repo: String,
    /// `release` or `branch`.
    follows: &'static str,
    branch: Option<String>,
    /// The commit builds use now.
    sha: String,
    tag: Option<String>,
    version: Option<String>,
    date: Option<Time>,
    /// Newer than the sources this Kiwi version pins.
    updated: bool,
}

/// The followed packages of an iOS recipe, with the sources builds use now.
pub fn sources(recipe: &Recipe, follow: &Follow, on: bool) -> Vec<SourceView> {
    if recipe.android() {
        return Vec::new();
    }
    followed(recipe)
        .map(|(package, spec, repo, base)| {
            let pin = follow.current.get(&package.id).filter(|_| on);
            SourceView {
                id: package.id.clone(),
                name: package
                    .name
                    .clone()
                    .unwrap_or_else(|| repo.rsplit('/').next().unwrap_or(repo).to_owned()),
                repo: repo.to_owned(),
                follows: if spec.release.is_some() {
                    "release"
                } else {
                    "branch"
                },
                branch: spec.branch.clone(),
                sha: pin.map_or(base, |p| p.sha.as_str()).to_owned(),
                tag: pin.and_then(|p| p.tag.clone()),
                version: pin.and_then(|p| p.version.clone()),
                date: pin.and_then(|p| p.date),
                updated: pin.is_some(),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pin(sha: char) -> Pin {
        Pin {
            sha: sha.to_string().repeat(40),
            base: "0".repeat(40),
            ..blank()
        }
    }

    fn pins(entries: &[(&str, char)]) -> Pins {
        entries
            .iter()
            .map(|(id, sha)| ((*id).to_owned(), pin(*sha)))
            .collect()
    }

    #[test]
    fn templates_capture_version_numbers() {
        let c = capture("v{app_version}-{version}", "v2026.45.0-1.3.8").unwrap();
        assert_eq!(c["app_version"], "2026.45.0");
        assert_eq!(c["version"], "1.3.8");
        let c = capture(
            "NeoFreeBird-sideloaded-X_{version}_{app_version}.ipa",
            "NeoFreeBird-sideloaded-X_7.3.1_12.39.ipa",
        )
        .unwrap();
        assert_eq!(
            (c["version"].as_str(), c["app_version"].as_str()),
            ("7.3.1", "12.39")
        );
        for (template, text) in [
            ("v{version}", "v7.3.1-beta"),
            ("v{version}", "v"),
            ("v{version}", "v1..2"),
            ("v{app_version}-{version}", "v2026.43.0"),
            ("v{version}", "x7.3.1"),
            ("v{version}", "v1.2.3.4.5"),
        ] {
            assert!(capture(template, text).is_none(), "{template} {text}");
        }
    }

    #[test]
    fn releases_name_the_app_version_they_support() {
        let reddit = ReleaseSpec {
            tag: "v{app_version}-{version}".into(),
            version_prefix: None,
            asset: None,
        };
        let releases = json!([
            {"tag_name": "v2026.46.0-1.4.0", "draft": true},
            {"tag_name": "v2026.46.0-1.3.9", "prerelease": true},
            {"tag_name": "nightly"},
            {"tag_name": "v2026.45.0-1.3.8"},
            {"tag_name": "v2026.44.0-1.3.7"}
        ]);
        let found = pick_release(&reddit, &releases).unwrap();
        assert_eq!(found.tag.as_deref(), Some("v2026.45.0-1.3.8"));
        assert_eq!(found.version.as_deref(), Some("1.3.8"));
        assert_eq!(found.app_version.as_deref(), Some("2026.45.0"));

        let x = ReleaseSpec {
            tag: "v{version}".into(),
            version_prefix: Some("7.".into()),
            asset: Some("NeoFreeBird-sideloaded-X_{version}_{app_version}.ipa".into()),
        };
        let releases = json!([
            {"tag_name": "v7.3.2", "assets": [{"name": "com.nfb.tweak_7.3.2.deb"}]},
            {"tag_name": "v6.10.1", "assets": [{"name": "NeoFreeBird-sideloaded-X_6.10.1_12.39.ipa"}]},
            {"tag_name": "v7.3.1", "assets": [
                {"name": "NeoFreeBird-sideloaded-X_7.3.0_12.38.1.ipa"},
                {"name": "NeoFreeBird-sideloaded-X_7.3.1_12.39.ipa"}]}
        ]);
        let found = pick_release(&x, &releases).unwrap();
        assert_eq!(
            found.tag.as_deref(),
            Some("v7.3.1"),
            "7.3.2 names no X version"
        );
        assert_eq!(found.app_version.as_deref(), Some("12.39"));
        assert!(pick_release(&x, &json!({"message": "rate limited"})).is_none());
    }

    #[test]
    fn candidates_become_current_once_they_build_and_are_given_up_after_failures() {
        let mut f = Follow::default();
        let newer = pins(&[("tweak", 'a')]);
        offer(&mut f, newer.clone());
        assert!(pending(&f, true) && !pending(&f, false));
        assert_eq!(for_build(&f, true), (newer.clone(), true));
        assert_eq!(for_build(&f, false), (Pins::new(), false));

        // A cancelled build and a lost Mac builder don't count.
        finish(&mut f, &newer, "cancelled", None, true);
        finish(&mut f, &newer, "failed", Some("lost"), false);
        assert_eq!(f.attempts, 0);
        finish(&mut f, &newer, "failed", Some("compile"), true);
        assert_eq!((f.attempts, f.candidate.is_some()), (1, true));
        finish(&mut f, &newer, "blocked", Some("compile"), true);
        assert_eq!(f.candidate, None);
        assert_eq!(f.failed.as_ref().unwrap().error, "compile");
        assert_eq!(
            for_build(&f, true),
            (Pins::new(), false),
            "builds use what last built"
        );

        // The same sources again are not retried; newer ones are.
        offer(&mut f, newer.clone());
        assert_eq!(f.candidate, None);
        let newest = pins(&[("tweak", 'b')]);
        offer(&mut f, newest.clone());
        assert_eq!(
            (f.candidate.as_ref(), f.failed.is_none()),
            (Some(&newest), true)
        );
        finish(&mut f, &newest, "ready", None, true);
        assert_eq!(
            (f.current.clone(), f.candidate.is_none()),
            (newest.clone(), true)
        );
        // Builds with the current sources keep them.
        finish(&mut f, &newest, "failed", Some("network"), true);
        assert_eq!(f.current, newest);
        offer(&mut f, newest);
        assert!(f.candidate.is_none());
    }

    #[test]
    fn every_followed_package_says_how_and_matches_its_recipe() {
        let mut count = 0;
        for recipe in recipes() {
            for (package, spec, repo, sha) in followed(recipe) {
                count += 1;
                assert!(!recipe.android(), "{}", recipe.id);
                assert!(repo.contains('/') && sha.len() == 40, "{}", package.id);
                assert!(
                    spec.branch.is_some() != spec.release.is_some(),
                    "{}",
                    package.id
                );
                if let Some(branch) = &spec.branch {
                    assert!(
                        branch
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b"._-/".contains(&b))
                    );
                }
                if let Some(r) = &spec.release {
                    assert!(r.tag.contains("{version}"), "{}", package.id);
                }
                // An app whose recipe pins its version learns newer ones from the releases.
                assert_eq!(
                    spec.names_app_version(),
                    recipe.app_version.is_some(),
                    "{}",
                    package.id
                );
            }
        }
        assert!(count >= 10);
        let reddit = super::super::recipe("reddit").unwrap();
        let current = Pins::from([(
            "redditfilter".to_owned(),
            Pin {
                sha: "f".repeat(40),
                base: reddit.packages[0].repo.as_ref().unwrap().1.clone(),
                tag: Some("v2026.46.0-1.3.9".into()),
                version: Some("1.3.9".into()),
                app_version: Some("2026.46.0".into()),
                ..blank()
            },
        )]);
        assert_eq!(
            request(reddit, &current).unwrap(),
            json!({"packages": {"redditfilter": "f".repeat(40)}, "app_version": "2026.46.0", "tweak": "RedditFilter 1.3.9"})
        );
        let follow = Follow {
            current: current.clone(),
            ..Follow::default()
        };
        assert_eq!(
            shown(reddit, &follow, true),
            ("RedditFilter 1.3.9".into(), Some("2026.46.0".into()))
        );
        assert_eq!(shown(reddit, &follow, false).0, reddit.tweak);
        // A Kiwi update that moves the recipe's pin drops what was found against the old one.
        let mut moved = follow.clone();
        moved.current.get_mut("redditfilter").unwrap().base = "0".repeat(40);
        prune(reddit, &mut moved);
        assert!(moved.current.is_empty());
        let mut kept = follow;
        prune(reddit, &mut kept);
        assert_eq!(kept.current, current);
    }
}
