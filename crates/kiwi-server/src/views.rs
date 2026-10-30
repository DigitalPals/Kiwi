//! Per-device JSON views of apps and builds (`AppSummary`, `AppDetail`, `Build` in api.md).

use std::{
    cmp::Ordering,
    collections::{HashMap, HashSet},
};

use chrono::Utc;
use kiwi_source::{DeviceProfile, Release};
use serde::Serialize;

use crate::{
    Kiwi,
    actions::{self, Action, Decision, INSTALLING_WINDOW, Inputs, UpdateReason},
    catalog::CatalogApp,
    db::{
        AppPrefs, BuildRow, BuildStatus, DeviceRow, InstallRow, OriginalChoice, StoreApp, StoreRow,
        Time,
    },
    devices::{device_profile, random_token},
    error::ApiResult,
    ota,
    signing::SigningInfo,
    store::StoreAppJson,
};

pub const ORPHAN_SOURCE_NAME: &str = "Removed source";

#[derive(Debug, Serialize)]
pub struct ReleaseJson {
    pub version: String,
    pub build: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
    pub date: Option<String>,
    pub notes: Option<String>,
    pub size: Option<u64>,
    pub min_os: Option<String>,
    pub max_os: Option<String>,
    pub compatible: bool,
    pub incompatibility: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct InstalledJson {
    pub version: String,
    pub build: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
    pub signed_bundle_id: String,
    pub origin: String,
    pub verified: bool,
    pub installed_at: Time,
    pub expires_at: Option<Time>,
}

#[derive(Debug, Serialize)]
pub struct BuildJson {
    pub id: String,
    pub source_id: String,
    pub bundle_id: String,
    pub version: String,
    pub build: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
    pub status: BuildStatus,
    pub step: Option<String>,
    pub progress: Option<f64>,
    pub error: Option<String>,
    pub signed_bundle_id: String,
    pub size: Option<i64>,
    pub expires_at: Option<Time>,
    pub dropped_entitlements: Vec<String>,
    pub created_at: Time,
    pub finished_at: Option<Time>,
    pub install_url: Option<String>,
}

/// The app of another source installed under the same signed bundle id; installing this
/// app replaces it.
#[derive(Debug, Serialize)]
pub struct ReplacesJson {
    pub name: String,
    pub source_name: String,
    pub version: String,
}

#[derive(Debug, Serialize)]
pub struct AppStateJson {
    pub action: Action,
    pub update_reason: Option<UpdateReason>,
    pub installed: Option<InstalledJson>,
    pub target: Option<ReleaseJson>,
    pub build: Option<BuildJson>,
    pub install_url: Option<String>,
    pub open_url: Option<String>,
    pub replaces: Option<ReplacesJson>,
}

/// How this app's version compares with the App Store's.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum VersionStatus {
    Same,
    /// The App Store has a newer version.
    Behind,
    Ahead,
    /// The version numbers cannot be compared.
    Unknown,
}

pub fn version_status(app_version: &str, store_version: &str) -> VersionStatus {
    match kiwi_source::compare_numeric(app_version, store_version) {
        Some(Ordering::Equal) => VersionStatus::Same,
        Some(Ordering::Less) => VersionStatus::Behind,
        Some(Ordering::Greater) => VersionStatus::Ahead,
        None => VersionStatus::Unknown,
    }
}

/// The App Store app an app is based on (`Original` in api.md).
#[derive(Debug, Serialize)]
pub struct OriginalJson {
    #[serde(flatten)]
    pub app: StoreAppJson,
    /// The version compared: the signed IPA's when built, else the release's.
    pub app_version: Option<String>,
    pub status: VersionStatus,
}

/// How an app's original was found (`AppDetail.original_match`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OriginalMatch {
    /// The App Store app with the same bundle id.
    BundleId,
    /// Chosen by hand.
    Linked,
    /// No App Store app has the bundle id (or the linked app is gone).
    NotFound,
    /// The user said it is not based on an App Store app.
    None,
    /// Not looked up yet.
    Pending,
}

#[derive(Debug, Serialize)]
pub struct AppSummary {
    pub source_id: String,
    pub source_name: String,
    pub bundle_id: String,
    pub name: String,
    pub developer: Option<String>,
    pub subtitle: Option<String>,
    pub icon_url: Option<String>,
    pub tint_color: Option<String>,
    pub category: Option<String>,
    pub unavailable: Option<String>,
    pub orphaned: bool,
    /// Not shown in Browse (hidden, or not picked from its source).
    pub hidden: bool,
    /// Appeared after its source was switched to picking apps; not chosen yet.
    pub is_new: bool,
    pub original: Option<OriginalJson>,
    pub state: AppStateJson,
}

/// Another source's version of the same app (same original, or same bundle id).
#[derive(Debug, Serialize)]
pub struct OtherVersionJson {
    pub source_id: String,
    pub source_name: String,
    pub bundle_id: String,
    pub name: String,
    pub icon_url: Option<String>,
    pub tint_color: Option<String>,
    pub version: Option<String>,
    pub installed: bool,
    pub hidden: bool,
}

#[derive(Debug, Serialize)]
pub struct Privacy {
    pub key: String,
    pub description: String,
}

#[derive(Debug, Serialize)]
pub struct PermissionsJson {
    pub entitlements: Vec<String>,
    pub privacy: Vec<Privacy>,
}

#[derive(Debug, Serialize)]
pub struct AppDetail {
    #[serde(flatten)]
    pub summary: AppSummary,
    pub description: Option<String>,
    pub screenshots: Vec<String>,
    pub releases: Vec<ReleaseJson>,
    pub permissions: PermissionsJson,
    pub dropped_entitlements: Vec<String>,
    pub signed_bundle_id: String,
    pub source_url: String,
    pub original_match: OriginalMatch,
    pub other_versions: Vec<OtherVersionJson>,
}

pub fn release_json(r: &Release, device: Option<&DeviceProfile>) -> ReleaseJson {
    let incompatibility = device.and_then(|d| r.incompatibility(d));
    ReleaseJson {
        version: r.version.clone(),
        build: r.build.clone(),
        revision: r.revision,
        date: r.date.clone(),
        notes: r.notes.clone(),
        size: r.size,
        min_os: r.min_os.clone(),
        max_os: r.max_os.clone(),
        compatible: incompatibility.is_none(),
        incompatibility,
    }
}

fn installed_json(i: &InstallRow) -> InstalledJson {
    InstalledJson {
        version: i.version.clone(),
        build: i.build.clone(),
        revision: i.release.as_ref().and_then(|r| r.revision),
        signed_bundle_id: i.signed_bundle_id.clone(),
        origin: i.origin.clone(),
        verified: i.verified,
        installed_at: i.installed_at,
        expires_at: i.expires_at,
    }
}

fn install_url(
    kiwi: &Kiwi,
    device_id: &str,
    public_url: &str,
    build: &BuildRow,
    now: Time,
) -> ApiResult<String> {
    let token = kiwi
        .db
        .attempt_token(&build.id, device_id, random_token, now)?;
    Ok(ota::install_url(public_url, &token))
}

/// One build needs no catalog, signing snapshot or unrelated device/app state. In
/// particular, progress polling must not reload the entire build history each time.
pub fn build_json(
    kiwi: &Kiwi,
    device_id: &str,
    public_url: &str,
    b: &BuildRow,
    now: Time,
) -> ApiResult<BuildJson> {
    Ok(BuildJson {
        id: b.id.clone(),
        source_id: b.source_id.clone(),
        bundle_id: b.bundle_id.clone(),
        version: b.version.clone(),
        build: b.build.clone(),
        revision: b.release.revision,
        status: b.status,
        step: b.step.clone(),
        progress: b.progress,
        error: b.error.clone(),
        signed_bundle_id: b.signed_bundle_id.clone(),
        size: b.size,
        expires_at: b.expires_at,
        dropped_entitlements: b.dropped_entitlements.clone(),
        created_at: b.created_at,
        finished_at: b.finished_at,
        install_url: if b.usable(now) {
            Some(install_url(kiwi, device_id, public_url, b, now)?)
        } else {
            None
        },
    })
}

/// Everything needed to compute app states for one device, loaded once per request.
pub struct View<'a> {
    kiwi: &'a Kiwi,
    pub device: DeviceRow,
    pub profile: Option<DeviceProfile>,
    public_url: String,
    builds: HashMap<(String, String), Vec<BuildRow>>,
    pub installs: Vec<InstallRow>,
    installs_by_signed_id: HashMap<String, usize>,
    /// Apps (source id, bundle id) with an install in progress on the device.
    installing: HashSet<(String, String)>,
    pub signing: SigningInfo,
    pub team: Option<String>,
    /// Choices per app (source id, bundle id).
    prefs: HashMap<(String, String), AppPrefs>,
    /// Cached App Store lookups of the current storefront by bundle id.
    store: HashMap<String, StoreRow>,
    pub now: Time,
}

impl<'a> View<'a> {
    pub fn load(kiwi: &'a Kiwi, device: &DeviceRow, public_url: &str) -> ApiResult<Self> {
        let now = Utc::now();
        let mut builds: HashMap<(String, String), Vec<BuildRow>> = HashMap::new();
        for b in kiwi.db.builds()? {
            builds
                .entry((b.source_id.clone(), b.bundle_id.clone()))
                .or_default()
                .push(b);
        }
        let installing = kiwi
            .db
            .device_attempts(&device.id)?
            .into_iter()
            .filter(|(_, a)| a.in_progress(now, INSTALLING_WINDOW))
            .map(|(app, _)| app)
            .collect();
        let installs = kiwi.db.installs(&device.id, false)?;
        let installs_by_signed_id = installs
            .iter()
            .enumerate()
            .map(|(index, install)| (install.signed_bundle_id.clone(), index))
            .collect();
        Ok(Self {
            kiwi,
            profile: device_profile(device),
            device: device.clone(),
            public_url: public_url.to_owned(),
            builds,
            installs,
            installs_by_signed_id,
            installing,
            signing: kiwi.signing_info(),
            team: kiwi.team_id(),
            prefs: kiwi.db.app_prefs()?,
            store: kiwi
                .db
                .store_rows(&kiwi.settings().app_store_country)?
                .into_iter()
                .map(|r| (r.bundle_id.clone(), r))
                .collect(),
            now,
        })
    }

    fn prefs(&self, app: &CatalogApp) -> Option<&AppPrefs> {
        self.prefs
            .get(&(app.source.id.clone(), app.app().bundle_id.clone()))
    }

    /// Not shown in Browse.
    pub fn hidden(&self, app: &CatalogApp) -> bool {
        !app.source.shows(self.prefs(app))
    }

    /// The App Store app this app is based on, and how it was found.
    pub fn original(&self, app: &CatalogApp) -> (OriginalMatch, Option<&StoreApp>) {
        let choice = self.prefs(app).map(|p| &p.original);
        let (wanted, linked) = match choice {
            None | Some(OriginalChoice::Auto) => (app.app().bundle_id.as_str(), false),
            Some(OriginalChoice::None) => return (OriginalMatch::None, None),
            Some(OriginalChoice::App(b)) => (b.as_str(), true),
        };
        match self.store.get(wanted).map(|r| r.app.as_ref()) {
            None => (OriginalMatch::Pending, None),
            Some(None) => (OriginalMatch::NotFound, None),
            Some(Some(store)) if linked => (OriginalMatch::Linked, Some(store)),
            Some(Some(store)) => (OriginalMatch::BundleId, Some(store)),
        }
    }

    /// Apps that are versions of the same app: the same App Store original, or (without
    /// one) the same bundle id.
    fn family(&self, app: &CatalogApp) -> String {
        self.original(app)
            .1
            .map_or(&app.app().bundle_id, |s| &s.bundle_id)
            .to_lowercase()
    }

    fn original_json(&self, app: &CatalogApp, d: &Decision<'_>) -> Option<OriginalJson> {
        let (_, store) = self.original(app);
        let store = store?;
        let a = app.app();
        let built = d
            .build
            .filter(|b| b.status == BuildStatus::Ready)
            .and_then(|b| b.app_version.clone());
        let app_version = built.or_else(|| {
            d.target
                .or((!a.releases.is_empty()).then_some(0))
                .map(|i| a.releases[i].version.clone())
        });
        Some(OriginalJson {
            app: StoreAppJson::from(store),
            status: app_version.as_deref().map_or(VersionStatus::Unknown, |v| {
                version_status(v, &store.version)
            }),
            app_version,
        })
    }

    pub fn signed_id(&self, bundle_id: &str) -> String {
        match &self.team {
            Some(team) => kiwi_signer::signed_bundle_id(bundle_id, team),
            None => bundle_id.to_owned(),
        }
    }

    /// The app's install: Kiwi's signed id of it, installed from this source (architecture.md
    /// "App actions"). Other sources often ship different apps with the same bundle id.
    pub fn installed_for(&self, app: &CatalogApp) -> Option<&InstallRow> {
        let a = app.app();
        let signed = self.signed_id(&a.bundle_id);
        self.installs_by_signed_id
            .get(&signed)
            .map(|&index| &self.installs[index])
            .filter(|i| i.source_id == app.source.id && i.bundle_id == a.bundle_id)
    }

    /// Another source's app installed under the signed id this app would use.
    pub fn replaced_by(&self, app: &CatalogApp) -> Option<&InstallRow> {
        let a = app.app();
        let signed = self.signed_id(&a.bundle_id);
        self.installs_by_signed_id
            .get(&signed)
            .map(|&index| &self.installs[index])
            .filter(|i| i.source_id != app.source.id || i.bundle_id != a.bundle_id)
    }

    fn source_name(&self, source_id: &str) -> String {
        self.kiwi.catalog.source(source_id).map_or_else(
            || ORPHAN_SOURCE_NAME.to_owned(),
            |s| s.source.catalog.name.clone(),
        )
    }

    pub fn app_builds(&self, source_id: &str, bundle_id: &str) -> &[BuildRow] {
        self.builds
            .get(&(source_id.to_owned(), bundle_id.to_owned()))
            .map_or(&[], Vec::as_slice)
    }

    pub fn decide<'v>(
        &'v self,
        app: &'v CatalogApp,
        installed: Option<&'v InstallRow>,
    ) -> Decision<'v> {
        let a = app.app();
        let signed = self.signed_id(&a.bundle_id);
        actions::decide(&Inputs {
            app: a,
            device: self.profile.as_ref(),
            installed,
            builds: self.app_builds(&app.source.id, &a.bundle_id),
            installing: self
                .installing
                .contains(&(app.source.id.clone(), a.bundle_id.clone())),
            // A signing plan clones a profile and its entitlements. Only installs
            // near expiry can use it; browsing uninstalled apps never needs one.
            resign_expiry: installed
                .filter(|i| i.origin == "kiwi")
                .and_then(|i| i.expires_at)
                .filter(|&expiry| expiry - self.now < actions::RESIGN_WINDOW)
                .and_then(|_| self.signing.plan(&signed, self.now).ok())
                .map(|p| p.expires_at),
            now: self.now,
        })
    }

    pub fn install_url(&self, build: &BuildRow) -> ApiResult<String> {
        install_url(
            self.kiwi,
            &self.device.id,
            &self.public_url,
            build,
            self.now,
        )
    }

    pub fn build_json(&self, b: &BuildRow) -> ApiResult<BuildJson> {
        build_json(self.kiwi, &self.device.id, &self.public_url, b, self.now)
    }

    /// `install` overrides the install row (library entries).
    pub fn summary(&self, app: &CatalogApp, install: Option<&InstallRow>) -> ApiResult<AppSummary> {
        let a = app.app();
        let installed = install.or_else(|| self.installed_for(app));
        let replaces = match installed {
            Some(_) => None,
            None => self.replaced_by(app).map(|i| ReplacesJson {
                name: i.name.clone(),
                source_name: self.source_name(&i.source_id),
                version: i.version.clone(),
            }),
        };
        let d = self.decide(app, installed);
        let unavailable = a.unavailable.clone().or_else(|| {
            d.target.is_none().then(|| {
                a.releases
                    .first()
                    .and_then(|r| self.profile.as_ref().and_then(|p| r.incompatibility(p)))
                    .unwrap_or_else(|| "No compatible release.".into())
            })
        });
        let build = d.build.map(|b| self.build_json(b)).transpose()?;
        // The decision's install build is the same ready build projected above;
        // reuse its URL instead of querying the attempt token twice per app.
        let install_url = d
            .install_build
            .and_then(|_| build.as_ref().and_then(|b| b.install_url.clone()));
        Ok(AppSummary {
            source_id: app.source.id.clone(),
            source_name: app.source.source.catalog.name.clone(),
            bundle_id: a.bundle_id.clone(),
            name: a.name.clone(),
            developer: a.developer.clone(),
            subtitle: a.subtitle.clone(),
            icon_url: a.icon_url.as_ref().map(|u| u.to_string()),
            tint_color: a.tint_color.clone(),
            category: a.category.clone(),
            unavailable,
            orphaned: false,
            hidden: self.hidden(app),
            is_new: app.source.is_new(self.prefs(app)),
            original: self.original_json(app, &d),
            state: AppStateJson {
                action: d.action,
                update_reason: d.update_reason,
                installed: installed.map(installed_json),
                target: d
                    .target
                    .map(|t| release_json(&a.releases[t], self.profile.as_ref())),
                build,
                install_url,
                open_url: d.open_url,
                replaces,
            },
        })
    }

    pub fn detail(&self, app: &CatalogApp) -> ApiResult<AppDetail> {
        let a = app.app();
        let summary = self.summary(app, None)?;
        let signed = self.signed_id(&a.bundle_id);
        let dropped = match summary.state.build.as_ref() {
            Some(b) if b.status == BuildStatus::Ready => b.dropped_entitlements.clone(),
            _ => self
                .signing
                .dropped_preview(&signed, &a.permissions.entitlements, self.now),
        };
        Ok(AppDetail {
            description: a.description.clone(),
            screenshots: a.screenshots.iter().map(|u| u.to_string()).collect(),
            releases: a
                .releases
                .iter()
                .map(|r| release_json(r, self.profile.as_ref()))
                .collect(),
            permissions: PermissionsJson {
                entitlements: a.permissions.entitlements.clone(),
                privacy: a
                    .permissions
                    .privacy
                    .iter()
                    .map(|(key, description)| Privacy {
                        key: key.clone(),
                        description: description.clone(),
                    })
                    .collect(),
            },
            dropped_entitlements: dropped,
            signed_bundle_id: signed,
            source_url: app.source.url.clone(),
            original_match: self.original(app).0,
            other_versions: self.other_versions(app),
            summary,
        })
    }

    /// Versions of the same app in other enabled sources (or under another bundle id).
    pub fn other_versions(&self, app: &CatalogApp) -> Vec<OtherVersionJson> {
        let family = self.family(app);
        self.kiwi
            .catalog
            .apps(None, None)
            .into_iter()
            .filter(|o| {
                (o.source.id != app.source.id || o.app().bundle_id != app.app().bundle_id)
                    && self.family(o) == family
            })
            .map(|o| {
                let a = o.app();
                let installed = self.installed_for(&o);
                let target = self.decide(&o, installed).target.or(Some(0));
                OtherVersionJson {
                    source_id: o.source.id.clone(),
                    source_name: o.source.source.catalog.name.clone(),
                    bundle_id: a.bundle_id.clone(),
                    name: a.name.clone(),
                    icon_url: a.icon_url.as_ref().map(|u| u.to_string()),
                    tint_color: a.tint_color.clone(),
                    version: target
                        .and_then(|i| a.releases.get(i))
                        .map(|r| r.version.clone()),
                    installed: installed.is_some(),
                    hidden: self.hidden(&o),
                }
            })
            .collect()
    }

    /// An install whose app is no longer in its source (orphaned), or one seen by devicectl
    /// under another bundle id (e.g. the App Store build): shown as installed, never updated.
    pub fn plain_summary(&self, install: &InstallRow, orphaned: bool) -> AppSummary {
        let source_name = if orphaned {
            ORPHAN_SOURCE_NAME.to_owned()
        } else {
            self.source_name(&install.source_id)
        };
        AppSummary {
            source_id: install.source_id.clone(),
            source_name,
            bundle_id: install.bundle_id.clone(),
            name: install.name.clone(),
            developer: install.developer.clone(),
            subtitle: None,
            icon_url: install.icon_url.clone(),
            tint_color: None,
            category: None,
            unavailable: None,
            orphaned,
            hidden: false,
            is_new: false,
            original: None,
            state: AppStateJson {
                action: Action::Open,
                update_reason: None,
                installed: Some(installed_json(install)),
                target: None,
                build: None,
                install_url: None,
                open_url: install
                    .url_scheme
                    .as_ref()
                    .filter(|_| install.origin == "kiwi")
                    .map(|s| format!("{s}://")),
                replaces: None,
            },
        }
    }

    /// Installed apps of enabled sources that have an update (new version or re-sign).
    pub fn updates(&self) -> Vec<(CatalogApp, UpdateReason)> {
        self.kiwi
            .catalog
            .apps(None, None)
            .into_iter()
            .filter_map(|app| {
                let installed = self.installed_for(&app)?;
                let reason = self.decide(&app, Some(installed)).update_reason?;
                Some((app, reason))
            })
            .collect()
    }

    pub fn library(&self) -> ApiResult<Vec<AppSummary>> {
        let mut out = Vec::new();
        for install in &self.installs {
            let app = self
                .kiwi
                .catalog
                .app(&install.source_id, &install.bundle_id);
            out.push(match app {
                Some(app) if install.signed_bundle_id == self.signed_id(&install.bundle_id) => {
                    self.summary(&app, Some(install))?
                }
                Some(_) => self.plain_summary(install, false),
                None => self.plain_summary(install, true),
            });
        }
        out.sort_by_cached_key(|s| s.name.to_lowercase());
        Ok(out)
    }
}
