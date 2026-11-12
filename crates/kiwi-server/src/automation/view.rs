//! Product-facing state derived from the recipes, the durable job history and the published
//! source.
use super::{AppState, Job, Limitation, Platform, Recipe, SOURCE_ID, recipe, recipes, store_icon};
use crate::{Kiwi, db::Time, error::ApiResult};
use chrono::{Duration, Utc};
use kiwi_source::Url;
use serde::Serialize;
use serde_json::Value;
use std::{collections::BTreeMap, io::Read};

#[derive(Serialize)]
pub struct Feature {
    id: String,
    name: String,
    description: String,
}

#[derive(Serialize)]
pub struct Published {
    version: String,
    build: Option<String>,
    revision: Option<u64>,
    date: Option<String>,
    tweaks: Option<Vec<String>>,
    name: Option<String>,
}

/// A job as the API shows it.
#[derive(Serialize)]
pub struct JobView {
    #[serde(flatten)]
    job: Job,
    /// Only on a queued job: how many builds run before it starts, the running one included
    /// (0 when it starts next and the worker is free).
    #[serde(skip_serializing_if = "Option::is_none")]
    ahead: Option<usize>,
}

fn job_views(jobs: Vec<Job>, ahead: &BTreeMap<String, usize>) -> Vec<JobView> {
    jobs.into_iter()
        .map(|job| JobView {
            ahead: ahead.get(&job.id).copied(),
            job,
        })
        .collect()
}

#[derive(Serialize)]
pub struct AppView {
    id: String,
    platform: Platform,
    name: String,
    bundle_id: String,
    icon_url: Option<String>,
    tweak: String,
    summary: String,
    experimental: bool,
    warning: Option<String>,
    names: Vec<String>,
    app_version: Option<String>,
    limitations: Vec<Limitation>,
    /// Packages always built in, with a description worth showing.
    included: Vec<Feature>,
    available_tweaks: Vec<Feature>,
    #[serde(flatten)]
    state: AppState,
    /// Newest first.
    jobs: Vec<JobView>,
    checks: Vec<Value>,
    published: Option<Published>,
    source_enabled: Option<bool>,
    configuration_changed: bool,
    next_check_at: Option<Time>,
    /// Only on community apps: the bundle, how it was chosen and any offered update.
    #[serde(skip_serializing_if = "Option::is_none")]
    community: Option<crate::community::View>,
    /// iOS packages Kiwi follows upstream, with the sources builds use now.
    tweak_sources: Vec<super::follow::SourceView>,
    /// Whether Kiwi follows tweak updates (Settings).
    follow_tweak_updates: bool,
}

#[derive(Serialize)]
pub struct Overview {
    apps: Vec<AppView>,
    checks: Vec<Value>,
}

/// Older published builds recorded their choices only in private provenance. Read only
/// those bounded, non-secret fields, without exposing the full document.
fn provenance_choices(
    kiwi: &Kiwi,
    recipe: &Recipe,
    sha: Option<&str>,
) -> Option<(Vec<String>, Option<String>)> {
    let sha = sha.filter(|s| super::valid_sha(s))?;
    let path = kiwi
        .config
        .data_dir
        .join("automation/artifacts")
        .join(format!("{sha}.json"));
    let mut bytes = Vec::new();
    std::fs::File::open(path)
        .ok()?
        .take(65537)
        .read_to_end(&mut bytes)
        .ok()?;
    if bytes.len() > 65536 {
        return None;
    }
    let value: Value = serde_json::from_slice(&bytes).ok()?;
    let tweaks: Vec<String> = serde_json::from_value(value["plan"]["tweaks"].clone()).ok()?;
    let name = value["plan"]["name"]
        .as_str()
        .filter(|n| recipe.names.iter().any(|known| known == n))
        .map(str::to_owned);
    tweaks
        .iter()
        .all(|t| recipe.optional().any(|p| &p.id == t))
        .then_some((tweaks, name))
}

fn features<'a>(packages: impl Iterator<Item = &'a super::Package>) -> Vec<Feature> {
    packages
        .filter_map(|p| {
            Some(Feature {
                id: p.id.clone(),
                name: p.name.clone()?,
                description: p.description.clone().unwrap_or_default(),
            })
        })
        .collect()
}

fn build_view(
    kiwi: &Kiwi,
    recipe: &Recipe,
    state: AppState,
    jobs: Vec<JobView>,
    checks: Vec<Value>,
) -> AppView {
    let source = kiwi.catalog.source(SOURCE_ID);
    let app = source.as_ref().and_then(|s| {
        s.source
            .catalog
            .apps
            .iter()
            .find(|a| a.bundle_id == recipe.bundle_id)
            .cloned()
    });
    let published = app.as_ref().and_then(|a| a.releases.first()).map(|r| {
        let recorded = provenance_choices(kiwi, recipe, r.sha256.as_deref());
        Published {
            version: r.version.clone(),
            build: r.build.clone(),
            revision: r.revision,
            date: r.date.clone(),
            tweaks: state
                .last_success_tweaks
                .clone()
                .or_else(|| recorded.as_ref().map(|c| c.0.clone())),
            name: state
                .last_success_name
                .clone()
                .or_else(|| recorded.and_then(|c| c.1)),
        }
    });
    let configuration_changed = published.as_ref().is_some_and(|p| {
        let tweaks = p.tweaks.as_ref().is_some_and(|tweaks| {
            let mut published = tweaks.clone();
            let mut selected = state.preferences.tweaks.clone();
            published.sort();
            selected.sort();
            published != selected
        });
        let name = !recipe.names.is_empty() && p.name.is_some() && p.name != state.preferences.name;
        tweaks || name
    });
    let next_check_at = (state.added
        && state.preferences.enabled
        && !jobs.iter().any(|j| j.job.active())
        && kiwi.automation.can_build(recipe))
    .then(|| {
        state.last_checked_at.map_or_else(Utc::now, |last| {
            last + Duration::minutes(i64::from(state.preferences.interval_minutes))
        })
    });
    let following = kiwi.settings().follow_tweak_updates;
    let (tweak, app_version) = super::follow::shown(recipe, &state.follow, following);
    let tweak_sources = super::follow::sources(recipe, &state.follow, following);
    AppView {
        id: recipe.id.clone(),
        platform: recipe.platform,
        name: recipe.name.clone(),
        bundle_id: recipe.bundle_id.clone(),
        icon_url: store_icon(kiwi, recipe.icon_id())
            .or_else(|| app.and_then(|a| a.icon_url.as_ref().map(Url::to_string)))
            .map(|u| crate::linked::client_icon(&u)),
        tweak,
        summary: recipe.summary.clone(),
        experimental: recipe.experimental,
        warning: recipe.warning.clone(),
        names: recipe.names.clone(),
        app_version,
        limitations: recipe.limitations.clone(),
        included: features(recipe.packages.iter().filter(|p| !p.optional)),
        available_tweaks: features(recipe.optional()),
        state,
        jobs,
        checks,
        published,
        source_enabled: source.map(|s| s.enabled),
        configuration_changed,
        next_check_at,
        community: None,
        tweak_sources,
        follow_tweak_updates: following,
    }
}

/// A community app's state, in the shape of a recipe's.
fn community_view(kiwi: &Kiwi, id: &str) -> ApiResult<AppView> {
    let mut state = kiwi.automation.snapshot();
    let ahead = kiwi.automation.ahead(&state);
    let added = state
        .community
        .remove(id)
        .ok_or_else(|| crate::error::ApiError::not_found("Kiwi doesn't build that app."))?;
    let app = state.apps.remove(id).unwrap_or_default();
    let jobs: Vec<Job> = state.jobs.into_iter().filter(|j| j.app == id).collect();
    let shown = crate::community::shown(&added, &jobs);
    let source = kiwi.catalog.source(SOURCE_ID);
    let published = added.pins.as_ref().and_then(|pins| {
        let release = source.as_ref().and_then(|s| {
            s.source
                .catalog
                .apps
                .iter()
                .find(|a| a.bundle_id == pins.output_package)
                .and_then(|a| a.releases.first().cloned())
        })?;
        Some(Published {
            version: release.version,
            build: release.build,
            revision: release.revision,
            date: release.date,
            tweaks: Some(Vec::new()),
            name: None,
        })
    });
    Ok(AppView {
        id: id.to_owned(),
        platform: Platform::Android,
        name: shown.name,
        bundle_id: shown.bundle_id,
        icon_url: shown.icon_url,
        tweak: shown.tweak,
        summary: shown.summary,
        experimental: true,
        warning: Some(shown.warning),
        names: Vec::new(),
        app_version: shown.app_version,
        limitations: shown.limitations,
        included: shown
            .included
            .into_iter()
            .map(|p| Feature {
                id: p.name.clone(),
                name: p.name,
                description: p.description,
            })
            .collect(),
        available_tweaks: Vec::new(),
        state: app,
        jobs: job_views(jobs, &ahead),
        checks: Vec::new(),
        published,
        source_enabled: source.map(|s| s.enabled),
        configuration_changed: false,
        next_check_at: None,
        community: Some(shown.view),
        tweak_sources: Vec::new(),
        follow_tweak_updates: false,
    })
}

/// One app's build settings, status and full job history.
pub fn app_view(kiwi: &Kiwi, id: &str) -> ApiResult<AppView> {
    if crate::community::is_id(id) {
        return community_view(kiwi, id);
    }
    let recipe = recipe(id)?;
    let mut state = kiwi.automation.snapshot();
    let checks = kiwi.automation.checks(&state);
    let ahead = kiwi.automation.ahead(&state);
    let app = state.apps.remove(id).unwrap_or_default();
    let jobs = state.jobs.into_iter().filter(|j| j.app == id).collect();
    Ok(build_view(
        kiwi,
        recipe,
        app,
        job_views(jobs, &ahead),
        checks,
    ))
}

/// Every app Kiwi can build, with its latest jobs, and the shared Mac setup checks.
pub fn overview(kiwi: &Kiwi) -> Overview {
    let mut state = kiwi.automation.snapshot();
    let checks = kiwi.automation.checks(&state);
    let ahead = kiwi.automation.ahead(&state);
    let apps = recipes()
        .iter()
        .map(|recipe| {
            let app = state.apps.remove(&recipe.id).unwrap_or_default();
            let jobs = state
                .jobs
                .iter()
                .filter(|j| j.app == recipe.id)
                .take(3)
                .cloned()
                .collect();
            build_view(kiwi, recipe, app, job_views(jobs, &ahead), Vec::new())
        })
        .collect();
    Overview { apps, checks }
}
