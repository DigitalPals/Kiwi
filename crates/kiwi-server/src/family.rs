//! The family (architecture.md "Family"): people (Tailscale users), the owner, which apps members
//! may use, enrollment of a device's UDID, the owner's approval and registration with
//! Apple through the App Store Connect API.

use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, RwLock},
    time::{Duration, Instant},
};

use axum::{
    body::Bytes,
    extract::State,
    http::{HeaderMap, HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use kiwi_appstore::{Client, Credentials};
use serde::Serialize;

use crate::{
    Kiwi,
    catalog::CatalogApp,
    db::{AppPrefs, DeviceRow, PersonRow, RegistrationRow, RegistrationStatus, Time},
    devices::random_token,
    error::{ApiError, ApiResult},
    push::{self, Payload},
    routes::P,
    signing::SigningInfo,
    views::View,
};

/// How long an enrollment profile can be installed after it was downloaded.
pub const ENROLLMENT_TTL: chrono::Duration = chrono::Duration::hours(1);
/// `tailscale status` is not asked again for the owner sooner than this.
const OWNER_RETRY: Duration = Duration::from_secs(30);
/// The scheduler retries a registration with Apple after this long.
const APPLE_RETRY: Duration = Duration::from_secs(10 * 60);
/// Apple's device names may have at most 50 characters.
const APPLE_NAME_LIMIT: usize = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    Owner,
    Member,
}

/// Family state that lives in memory.
pub struct Family {
    /// `--owner`; when set, Tailscale is not asked.
    configured_owner: Option<String>,
    owner: RwLock<Option<String>>,
    owner_checked: Mutex<Option<Instant>>,
    /// UDIDs of approved devices: profiles that include more of them are preferred.
    required: RwLock<Vec<String>>,
    /// One registration with Apple at a time.
    apple: tokio::sync::Mutex<()>,
    apple_attempt: Mutex<Option<Instant>>,
    /// Base URL of the App Store Connect API ([`kiwi_appstore::DEFAULT_URL`]).
    pub appstore_url: String,
}

impl Family {
    pub fn new(
        configured_owner: Option<String>,
        saved_owner: Option<String>,
        appstore_url: String,
    ) -> Self {
        Self {
            owner: RwLock::new(configured_owner.clone().or(saved_owner)),
            configured_owner,
            owner_checked: Mutex::default(),
            required: RwLock::default(),
            apple: tokio::sync::Mutex::new(()),
            apple_attempt: Mutex::default(),
            appstore_url,
        }
    }

    /// The owner's Tailscale login, when known.
    pub fn owner(&self) -> Option<String> {
        self.owner.read().unwrap().clone()
    }

    pub fn required_devices(&self) -> Vec<String> {
        self.required.read().unwrap().clone()
    }
}

// ---- the owner ----

/// Asks Tailscale who owns the Mac (at most every 30 s unless `force`), unless `--owner`
/// names them. The answer is saved, so Kiwi still knows the owner while Tailscale is down.
pub async fn refresh_owner(kiwi: &Kiwi, force: bool) {
    if kiwi.family.configured_owner.is_some() {
        return;
    }
    {
        let mut checked = kiwi.family.owner_checked.lock().unwrap();
        if !force && checked.is_some_and(|at| at.elapsed() < OWNER_RETRY) {
            return;
        }
        *checked = Some(Instant::now());
    }
    match kiwi.devices.owner().await {
        Ok(Some(user)) => {
            let changed = kiwi.family.owner().as_ref() != Some(&user.login);
            if changed {
                tracing::info!(
                    "the Mac belongs to {} in Tailscale: they own Kiwi",
                    user.login
                );
                if let Err(e) = kiwi.db.set_meta("owner_login", &user.login) {
                    tracing::warn!("could not save the owner: {e}");
                }
                *kiwi.family.owner.write().unwrap() = Some(user.login.clone());
            }
            let fallback = login_name(&user.login);
            if let Err(e) =
                kiwi.db
                    .ensure_person(&user.login, user.name.as_deref(), &fallback, Utc::now())
            {
                tracing::warn!("could not save the owner: {e}");
            }
        }
        Ok(None) => tracing::warn!(
            "the Mac belongs to no Tailscale user (a tagged machine?): start Kiwi with --owner <login>"
        ),
        Err(e) => tracing::warn!("could not ask Tailscale who owns the Mac: {e}"),
    }
}

/// "noah" for "noah@example.com".
fn login_name(login: &str) -> String {
    login.split('@').next().unwrap_or(login).to_owned()
}

fn first_name(name: &str) -> &str {
    name.split_whitespace().next().unwrap_or(name)
}

/// Owner for browser devices on the Mac and devices from before people existed; member
/// for every other person (also while the owner is unknown).
pub fn role(kiwi: &Kiwi, device: &DeviceRow) -> Role {
    match &device.person_id {
        None => Role::Owner,
        Some(person) if kiwi.family.owner().as_ref() == Some(person) => Role::Owner,
        Some(_) => Role::Member,
    }
}

/// The owner's first name ("Alex"), for "Apps from Alex".
pub fn owner_name(kiwi: &Kiwi) -> String {
    let Some(login) = kiwi.family.owner() else {
        return "the owner".into();
    };
    let name = kiwi
        .db
        .person(&login)
        .ok()
        .flatten()
        .map_or_else(|| login_name(&login), |p| p.name);
    first_name(&name).to_owned()
}

/// The owner's devices (and browsers on the Mac).
fn owner_devices(kiwi: &Kiwi) -> ApiResult<Vec<DeviceRow>> {
    Ok(kiwi
        .db
        .devices()?
        .into_iter()
        .filter(|d| role(kiwi, d) == Role::Owner)
        .collect())
}

// ---- access ----

/// What a device's person may do and use.
#[derive(Debug, Clone)]
pub struct Access {
    pub role: Role,
    pub person: Option<PersonRow>,
    choices: HashMap<(String, String), bool>,
    pub registration: Option<RegistrationRow>,
    /// UDID builds must include: the approved registration's (devicectl's is only shown,
    /// so a linked phone keeps installing as before).
    pub udid: Option<String>,
    /// May install apps: always for the owner; for members once approved and in the profile.
    pub ready: bool,
}

/// Whether the wildcard profile Kiwi signs with includes the device.
pub fn in_profile(signing: &SigningInfo, udid: &str, now: Time) -> bool {
    signing
        .active(now)
        .is_some_and(|plan| plan.profile.covers(udid))
}

pub fn access(
    kiwi: &Kiwi,
    device: &DeviceRow,
    signing: &SigningInfo,
    now: Time,
) -> ApiResult<Access> {
    let role = role(kiwi, device);
    let person = match &device.person_id {
        Some(id) => kiwi.db.person(id)?,
        None => None,
    };
    let choices = match (&person, role) {
        (Some(p), Role::Member) => kiwi.db.app_access(&p.id)?,
        _ => HashMap::new(),
    };
    let registration = kiwi.db.registration(&device.id)?;
    let approved = registration
        .as_ref()
        .filter(|r| r.status == RegistrationStatus::Approved);
    let udid = approved.map(|r| r.udid.clone());
    let ready = role == Role::Owner || approved.is_some_and(|r| in_profile(signing, &r.udid, now));
    Ok(Access {
        role,
        person,
        choices,
        registration,
        udid,
        ready,
    })
}

/// Whether `person` may use an app, and whether that was chosen by hand: an explicit
/// choice, else "Apps You Add Later" for apps first seen after it was turned on.
pub fn allowed(
    person: &PersonRow,
    choices: &HashMap<(String, String), bool>,
    source_id: &str,
    bundle_id: &str,
    prefs: Option<&AppPrefs>,
) -> (bool, bool) {
    if let Some(&allowed) = choices.get(&(source_id.to_owned(), bundle_id.to_owned())) {
        return (allowed, true);
    }
    let later = person.new_apps
        && person
            .new_apps_since
            .zip(prefs.map(|p| p.first_seen))
            .is_some_and(|(since, seen)| seen > since);
    (later, false)
}

impl Access {
    pub fn owner() -> Self {
        Self {
            role: Role::Owner,
            person: None,
            choices: HashMap::new(),
            registration: None,
            udid: None,
            ready: true,
        }
    }

    pub fn is_owner(&self) -> bool {
        self.role == Role::Owner
    }

    /// Whether this device may see and install the app.
    pub fn may_use(&self, app: &CatalogApp, prefs: Option<&AppPrefs>) -> bool {
        if self.is_owner() {
            return true;
        }
        self.ready
            && self.person.as_ref().is_some_and(|p| {
                allowed(
                    p,
                    &self.choices,
                    &app.source.id,
                    &app.app().bundle_id,
                    prefs,
                )
                .0
            })
    }

    pub fn require_owner(&self, kiwi: &Kiwi) -> ApiResult<()> {
        if self.is_owner() {
            Ok(())
        } else {
            Err(ApiError::forbidden(format!(
                "Only {} can do this.",
                owner_name(kiwi)
            )))
        }
    }

    pub fn require_ready(&self, kiwi: &Kiwi) -> ApiResult<()> {
        if self.ready {
            Ok(())
        } else {
            Err(ApiError::not_registered(format!(
                "This iPhone isn't registered yet. Register it in Kiwi; {} approves it.",
                owner_name(kiwi)
            )))
        }
    }
}

/// UDIDs of approved devices (cached for signing plans).
pub fn refresh_required(kiwi: &Kiwi) -> ApiResult<()> {
    let mut udids: Vec<String> = kiwi
        .db
        .registrations()?
        .into_iter()
        .filter(|r| r.status == RegistrationStatus::Approved)
        .map(|r| r.udid)
        .collect();
    udids.sort();
    udids.dedup();
    *kiwi.family.required.write().unwrap() = udids;
    Ok(())
}

// ---- JSON ----

#[derive(Debug, Serialize)]
pub struct RegistrationJson {
    pub status: &'static str,
    pub udid: Option<String>,
    pub product: Option<String>,
    pub os_build: Option<String>,
    pub requested_at: Option<Time>,
    pub decided_at: Option<Time>,
    pub apple_registered: bool,
    pub in_profile: bool,
    pub ready: bool,
    pub error: Option<String>,
}

pub fn registration_json(
    role: Role,
    device: &DeviceRow,
    reg: Option<&RegistrationRow>,
    signing: &SigningInfo,
    now: Time,
    owner_view: bool,
) -> RegistrationJson {
    let udid = reg.map(|r| r.udid.clone()).or_else(|| device.udid.clone());
    let in_profile = udid.as_deref().is_some_and(|u| in_profile(signing, u, now));
    let approved = reg.is_some_and(|r| r.status == RegistrationStatus::Approved);
    RegistrationJson {
        status: reg.map_or("none", |r| r.status.as_str()),
        product: reg
            .and_then(|r| r.product.clone())
            .or_else(|| device.model.clone()),
        os_build: reg.and_then(|r| r.os_build.clone()),
        requested_at: reg.map(|r| r.requested_at),
        decided_at: reg.and_then(|r| r.decided_at),
        apple_registered: in_profile || reg.is_some_and(|r| r.apple_id.is_some()),
        in_profile,
        ready: role == Role::Owner || (approved && in_profile),
        error: reg.and_then(|r| r.error.clone()).filter(|_| owner_view),
        udid,
    }
}

#[derive(Debug, Serialize)]
pub struct AccessJson {
    pub role: Role,
    pub person: Option<PersonRef>,
    pub owner_name: String,
}

#[derive(Debug, Serialize)]
pub struct PersonRef {
    pub id: String,
    pub name: String,
}

pub fn access_json(kiwi: &Kiwi, access: &Access) -> AccessJson {
    AccessJson {
        role: access.role,
        person: access.person.as_ref().map(|p| PersonRef {
            id: p.id.clone(),
            name: p.name.clone(),
        }),
        owner_name: owner_name(kiwi),
    }
}

#[derive(Debug, Serialize)]
pub struct FamilyDeviceJson {
    pub id: String,
    pub name: String,
    pub person_id: Option<String>,
    pub model: Option<String>,
    pub os_version: Option<String>,
    pub last_seen: Time,
    pub registration: RegistrationJson,
}

#[derive(Debug, Serialize)]
pub struct PersonJson {
    pub id: String,
    pub name: String,
    pub role: Role,
    pub new_apps: bool,
    pub app_count: usize,
    pub devices: Vec<FamilyDeviceJson>,
    pub last_seen: Time,
}

#[derive(Debug, Serialize)]
pub struct PersonAppJson {
    pub source_id: String,
    pub source_name: String,
    pub bundle_id: String,
    pub name: String,
    pub subtitle: Option<String>,
    pub icon_url: Option<String>,
    pub tint_color: Option<String>,
    pub allowed: bool,
    pub explicit: bool,
    pub installed_version: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PersonDetailJson {
    #[serde(flatten)]
    pub person: PersonJson,
    pub apps: Vec<PersonAppJson>,
}

#[derive(Debug, Serialize)]
pub struct AppAccessJson {
    pub person_id: String,
    pub name: String,
    pub role: Role,
    pub allowed: bool,
    pub installed_version: Option<String>,
    pub pending: bool,
}

#[derive(Debug, Serialize)]
pub struct ProfileSummary {
    pub uuid: String,
    pub name: String,
    pub devices: usize,
    pub expires_at: Time,
    pub kiwi: bool,
}

#[derive(Debug, Serialize)]
pub struct AppleJson {
    pub connected: bool,
    pub issuer_id: Option<String>,
    pub key_id: Option<String>,
    pub devices: Option<usize>,
    pub device_limit: usize,
    pub checked_at: Option<Time>,
    pub error: Option<String>,
    pub profile: Option<ProfileSummary>,
}

#[derive(Debug, Serialize)]
pub struct FamilyJson {
    pub people: Vec<PersonJson>,
    pub pending: usize,
    pub apple: AppleJson,
}

/// A device's display name (as in `Device.name`).
fn device_name(d: &DeviceRow) -> String {
    crate::devices::device_json(d).name
}

fn family_device(
    kiwi: &Kiwi,
    d: &DeviceRow,
    reg: Option<&RegistrationRow>,
    signing: &SigningInfo,
    now: Time,
) -> FamilyDeviceJson {
    FamilyDeviceJson {
        id: d.id.clone(),
        name: device_name(d),
        person_id: d.person_id.clone(),
        model: reg
            .and_then(|r| r.product.clone())
            .or_else(|| d.model.clone()),
        os_version: d.os_version.clone(),
        last_seen: d.last_seen,
        registration: registration_json(role(kiwi, d), d, reg, signing, now, true),
    }
}

/// Everything the family pages need, loaded once.
struct Snapshot {
    people: Vec<PersonRow>,
    devices: Vec<DeviceRow>,
    registrations: HashMap<String, RegistrationRow>,
    signing: SigningInfo,
    owner: Option<String>,
    now: Time,
}

impl Snapshot {
    fn load(kiwi: &Kiwi) -> ApiResult<Self> {
        let owner = kiwi.family.owner();
        let mut people = kiwi.db.people()?;
        if let Some(login) = &owner
            && !people.iter().any(|p| &p.id == login)
        {
            let now = Utc::now();
            people.push(PersonRow {
                id: login.clone(),
                name: login_name(login),
                renamed: false,
                new_apps: false,
                new_apps_since: None,
                created_at: now,
                last_seen: now,
            });
        }
        Ok(Self {
            people,
            devices: kiwi.db.devices()?,
            registrations: kiwi
                .db
                .registrations()?
                .into_iter()
                .map(|r| (r.device_id.clone(), r))
                .collect(),
            signing: kiwi.signing_info(),
            owner,
            now: Utc::now(),
        })
    }

    fn role(&self, person: &PersonRow) -> Role {
        if self.owner.as_ref() == Some(&person.id) {
            Role::Owner
        } else {
            Role::Member
        }
    }

    /// The person's Tailscale devices (for the owner also the legacy ones without a person).
    fn devices_of(&self, person: &PersonRow) -> Vec<&DeviceRow> {
        let owner = self.role(person) == Role::Owner;
        self.devices
            .iter()
            .filter(|d| {
                d.kind == "tailscale"
                    && match &d.person_id {
                        Some(p) => *p == person.id,
                        None => owner,
                    }
            })
            .collect()
    }

    fn person_json(&self, kiwi: &Kiwi, person: &PersonRow, app_count: usize) -> PersonJson {
        let devices = self.devices_of(person);
        PersonJson {
            id: person.id.clone(),
            name: person.name.clone(),
            role: self.role(person),
            new_apps: person.new_apps,
            app_count,
            last_seen: devices
                .iter()
                .map(|d| d.last_seen)
                .max()
                .unwrap_or(person.last_seen)
                .max(person.last_seen),
            devices: devices
                .into_iter()
                .map(|d| {
                    family_device(
                        kiwi,
                        d,
                        self.registrations.get(&d.id),
                        &self.signing,
                        self.now,
                    )
                })
                .collect(),
        }
    }
}

/// Apps of the enabled sources a person may use (all for the owner), with installs on
/// their devices.
fn person_apps(
    kiwi: &Kiwi,
    snapshot: &Snapshot,
    person: &PersonRow,
) -> ApiResult<Vec<PersonAppJson>> {
    let owner = snapshot.role(person) == Role::Owner;
    let choices = kiwi.db.app_access(&person.id)?;
    let prefs = kiwi.db.app_prefs()?;
    let mut installed: HashMap<(String, String), String> = HashMap::new();
    for d in snapshot.devices_of(person) {
        for i in kiwi.db.installs(&d.id, false)? {
            installed.insert((i.source_id, i.bundle_id), i.version);
        }
    }
    let mut apps: Vec<PersonAppJson> = kiwi
        .catalog
        .apps(None, None)
        .iter()
        .map(|app| {
            let a = app.app();
            let key = (app.source.id.clone(), a.bundle_id.clone());
            let (allowed, explicit) = if owner {
                (true, false)
            } else {
                allowed(person, &choices, &key.0, &key.1, prefs.get(&key))
            };
            PersonAppJson {
                source_id: app.source.id.clone(),
                source_name: app.source.source.catalog.name.clone(),
                bundle_id: a.bundle_id.clone(),
                name: a.name.clone(),
                subtitle: a.subtitle.clone(),
                icon_url: a.icon_url.as_ref().map(|u| u.to_string()),
                tint_color: a.tint_color.clone(),
                allowed,
                explicit,
                installed_version: installed.get(&key).cloned(),
            }
        })
        .collect();
    apps.sort_by_cached_key(|a| (a.name.to_lowercase(), a.source_name.to_lowercase()));
    Ok(apps)
}

fn sorted_people(snapshot: &Snapshot) -> Vec<&PersonRow> {
    let mut people: Vec<&PersonRow> = snapshot.people.iter().collect();
    people.sort_by_cached_key(|p| (snapshot.role(p) != Role::Owner, p.name.to_lowercase()));
    people
}

pub fn family_json(kiwi: &Kiwi) -> ApiResult<FamilyJson> {
    let snapshot = Snapshot::load(kiwi)?;
    let mut people = Vec::new();
    for person in sorted_people(&snapshot) {
        let count = person_apps(kiwi, &snapshot, person)?
            .iter()
            .filter(|a| a.allowed)
            .count();
        // Members only appear once they have a device (people seen through a browser on the
        // Mac or whose devices were all removed have nothing to manage).
        if snapshot.role(person) == Role::Member && snapshot.devices_of(person).is_empty() {
            continue;
        }
        people.push(snapshot.person_json(kiwi, person, count));
    }
    Ok(FamilyJson {
        pending: snapshot
            .registrations
            .values()
            .filter(|r| r.status == RegistrationStatus::Pending)
            .count(),
        people,
        apple: apple_json(kiwi)?,
    })
}

fn find_person<'a>(snapshot: &'a Snapshot, id: &str) -> ApiResult<&'a PersonRow> {
    snapshot
        .people
        .iter()
        .find(|p| p.id == id)
        .ok_or_else(|| ApiError::not_found("Person not found."))
}

pub fn person_json(kiwi: &Kiwi, id: &str) -> ApiResult<PersonDetailJson> {
    let snapshot = Snapshot::load(kiwi)?;
    let person = find_person(&snapshot, id)?;
    let apps = person_apps(kiwi, &snapshot, person)?;
    let count = apps.iter().filter(|a| a.allowed).count();
    Ok(PersonDetailJson {
        person: snapshot.person_json(kiwi, person, count),
        apps,
    })
}

/// Who may install an app (owner views of an app's page).
pub fn app_access(kiwi: &Kiwi, app: &CatalogApp) -> ApiResult<Vec<AppAccessJson>> {
    let snapshot = Snapshot::load(kiwi)?;
    let prefs = kiwi.db.app_prefs()?;
    let key = (app.source.id.clone(), app.app().bundle_id.clone());
    let mut out = Vec::new();
    for person in sorted_people(&snapshot) {
        let devices = snapshot.devices_of(person);
        let role = snapshot.role(person);
        if role == Role::Member && devices.is_empty() {
            continue;
        }
        let allowed = role == Role::Owner
            || allowed(
                person,
                &kiwi.db.app_access(&person.id)?,
                &key.0,
                &key.1,
                prefs.get(&key),
            )
            .0;
        let mut installed_version = None;
        for d in &devices {
            if let Some(i) = kiwi
                .db
                .installs(&d.id, false)?
                .into_iter()
                .find(|i| i.source_id == key.0 && i.bundle_id == key.1)
            {
                installed_version = Some(i.version);
                break;
            }
        }
        let pending = role == Role::Member
            && !devices.iter().any(|d| {
                snapshot.registrations.get(&d.id).is_some_and(|r| {
                    r.status == RegistrationStatus::Approved
                        && in_profile(&snapshot.signing, &r.udid, snapshot.now)
                })
            });
        out.push(AppAccessJson {
            person_id: person.id.clone(),
            name: person.name.clone(),
            role,
            allowed,
            installed_version,
            pending,
        });
    }
    Ok(out)
}

pub fn device_json(kiwi: &Kiwi, id: &str) -> ApiResult<FamilyDeviceJson> {
    let d = kiwi
        .db
        .device(id)?
        .ok_or_else(|| ApiError::not_found("Device not found."))?;
    let reg = kiwi.db.registration(id)?;
    Ok(family_device(
        kiwi,
        &d,
        reg.as_ref(),
        &kiwi.signing_info(),
        Utc::now(),
    ))
}

// ---- people ----

pub fn rename(kiwi: &Kiwi, id: &str, name: &str) -> ApiResult<()> {
    let name = name.trim();
    if name.is_empty() || name.chars().count() > 64 {
        return Err(ApiError::bad_request("Names have 1 to 64 characters."));
    }
    if !kiwi.db.rename_person(id, name)? {
        // The owner seen only through `tailscale status` so far.
        if kiwi.family.owner().as_deref() == Some(id) {
            kiwi.db.ensure_person(id, Some(name), name, Utc::now())?;
            kiwi.db.rename_person(id, name)?;
        } else {
            return Err(ApiError::not_found("Person not found."));
        }
    }
    Ok(())
}

fn member(kiwi: &Kiwi, id: &str) -> ApiResult<PersonRow> {
    if kiwi.family.owner().as_deref() == Some(id) {
        return Err(ApiError::bad_request("The owner always has every app."));
    }
    kiwi.db
        .person(id)?
        .ok_or_else(|| ApiError::not_found("Person not found."))
}

pub fn set_new_apps(kiwi: &Kiwi, id: &str, on: bool) -> ApiResult<()> {
    member(kiwi, id)?;
    kiwi.db.set_person_new_apps(id, on, Utc::now())?;
    Ok(())
}

pub fn set_apps(kiwi: &Kiwi, id: &str, apps: &[(String, String, bool)]) -> ApiResult<()> {
    member(kiwi, id)?;
    let now = Utc::now();
    for (source_id, bundle_id, allowed) in apps {
        if kiwi.catalog.app(source_id, bundle_id).is_none() {
            return Err(ApiError::bad_request(format!(
                "{bundle_id} is not an app of an enabled source."
            )));
        }
        kiwi.db
            .set_app_access(id, source_id, bundle_id, *allowed, now)?;
    }
    kiwi.kick.notify_one();
    Ok(())
}

pub fn remove_person(kiwi: &Kiwi, id: &str) -> ApiResult<()> {
    member(kiwi, id)?;
    kiwi.db.delete_person(id)?;
    refresh_required(kiwi)
}

// ---- enrollment ----

/// A one-time enrollment profile URL (path) for a device.
pub fn start_enrollment(kiwi: &Kiwi, device: &DeviceRow) -> ApiResult<String> {
    if device.kind != "tailscale" {
        return Err(ApiError::bad_request(
            "Open Kiwi on the iPhone through Tailscale to register it.",
        ));
    }
    let now = Utc::now();
    kiwi.db.prune_enrollments(now - ENROLLMENT_TTL)?;
    let token = random_token();
    kiwi.db.create_enrollment(&token, &device.id, now)?;
    Ok(format!("/enroll/{token}/kiwi.mobileconfig"))
}

/// A random version 4 UUID, upper case (as profiles use them).
fn uuid4() -> String {
    let mut b = [0u8; 16];
    getrandom::fill(&mut b).expect("system random number generator");
    b[6] = (b[6] & 0x0f) | 0x40;
    b[8] = (b[8] & 0x3f) | 0x80;
    let h = hex::encode_upper(b);
    format!(
        "{}-{}-{}-{}-{}",
        &h[..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..]
    )
}

/// The Profile Service payload: iOS sends the listed device attributes to `<public>/enroll/<token>`.
pub fn mobileconfig(public_url: &str, token: &str) -> Vec<u8> {
    use plist::{Dictionary, Value};
    let host = public_url
        .split("://")
        .nth(1)
        .unwrap_or(public_url)
        .trim_end_matches('/');
    let mut content = Dictionary::new();
    content.insert(
        "URL".into(),
        Value::String(format!("{public_url}/enroll/{token}")),
    );
    content.insert(
        "DeviceAttributes".into(),
        Value::Array(
            ["UDID", "PRODUCT", "VERSION", "DEVICE_NAME"]
                .into_iter()
                .map(|a| Value::String(a.into()))
                .collect(),
        ),
    );
    content.insert("Challenge".into(), Value::String(token.into()));
    let mut root = Dictionary::new();
    root.insert("PayloadContent".into(), Value::Dictionary(content));
    root.insert("PayloadOrganization".into(), Value::String("Kiwi".into()));
    root.insert(
        "PayloadDisplayName".into(),
        Value::String("Register this iPhone with Kiwi".into()),
    );
    root.insert(
        "PayloadDescription".into(),
        Value::String(format!(
            "Sends this iPhone's ID (UDID), model and iOS version to Kiwi at {host} so apps can be installed on it. Nothing stays installed."
        )),
    );
    root.insert(
        "PayloadIdentifier".into(),
        Value::String("local.kiwi.enroll".into()),
    );
    root.insert(
        "PayloadType".into(),
        Value::String("Profile Service".into()),
    );
    root.insert("PayloadUUID".into(), Value::String(uuid4()));
    root.insert("PayloadVersion".into(), Value::Integer(1.into()));
    let mut out = Vec::new();
    plist::to_writer_xml(&mut out, &Value::Dictionary(root)).expect("plist to memory");
    out
}

/// A UDID: 40 hex digits (older devices) or `XXXXXXXX-XXXXXXXXXXXXXXXX`.
pub fn valid_udid(udid: &str) -> bool {
    let hex = |s: &str| s.bytes().all(|b| b.is_ascii_hexdigit());
    match udid.split_once('-') {
        Some((a, b)) => a.len() == 8 && b.len() == 16 && hex(a) && hex(b),
        None => udid.len() == 40 && hex(udid),
    }
}

/// "iPhone" for "iPhone16,2", "iPad" for "iPad14,1".
fn device_kind(product: Option<&str>) -> &'static str {
    match product {
        Some(p) if p.starts_with("iPad") => "iPad",
        Some(p) if p.starts_with("iPod") => "iPod touch",
        _ => "iPhone",
    }
}

fn text_field(dict: &plist::Dictionary, key: &str) -> Option<String> {
    dict.get(key)
        .and_then(plist::Value::as_string)
        .map(|s| s.trim().chars().take(100).collect::<String>())
        .filter(|s| !s.is_empty())
}

fn page(status: StatusCode, message: &str) -> Response {
    (
        status,
        [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
        message.to_owned(),
    )
        .into_response()
}

/// `GET /enroll/:token/kiwi.mobileconfig`
pub async fn mobileconfig_handler(
    State(kiwi): State<Arc<Kiwi>>,
    P(token): P<String>,
    headers: HeaderMap,
) -> Response {
    match kiwi.db.enrollment(&token, Utc::now() - ENROLLMENT_TTL) {
        Ok(Some(_)) => {}
        Ok(None) => {
            return page(
                StatusCode::NOT_FOUND,
                "This registration link expired. Open Kiwi and tap Register This iPhone again.",
            );
        }
        Err(e) => return ApiError::from(e).into_response(),
    }
    let body = mobileconfig(&kiwi.public_url(&headers), &token);
    let mut response = (StatusCode::OK, body).into_response();
    let h = response.headers_mut();
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/x-apple-aspen-config"),
    );
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    h.insert(
        header::CONTENT_DISPOSITION,
        HeaderValue::from_static("attachment; filename=\"Kiwi.mobileconfig\""),
    );
    response
}

/// `POST /enroll/:token`: iOS sends the device attributes after the profile is installed.
pub async fn enroll_handler(
    State(kiwi): State<Arc<Kiwi>>,
    P(token): P<String>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    let now = Utc::now();
    let device_id = match kiwi.db.enrollment(&token, now - ENROLLMENT_TTL) {
        Ok(Some(device)) => device,
        Ok(None) => {
            return page(
                StatusCode::BAD_REQUEST,
                "This registration link expired. Open Kiwi and tap Register This iPhone again.",
            );
        }
        Err(e) => return ApiError::from(e).into_response(),
    };
    let dict = match kiwi_signer::signed_plist(&body) {
        Ok(dict) => dict,
        Err(e) => {
            tracing::warn!("unreadable enrollment from {device_id}: {e}");
            return page(
                StatusCode::BAD_REQUEST,
                "The iPhone's answer could not be read.",
            );
        }
    };
    if text_field(&dict, "CHALLENGE").is_some_and(|c| c != token) {
        return page(
            StatusCode::BAD_REQUEST,
            "The iPhone's answer does not match this link.",
        );
    }
    let Some(udid) = text_field(&dict, "UDID").filter(|u| valid_udid(u)) else {
        return page(
            StatusCode::BAD_REQUEST,
            "The iPhone did not send a valid UDID.",
        );
    };
    let enrolled = Enrolled {
        udid: udid.to_uppercase(),
        product: text_field(&dict, "PRODUCT"),
        os_build: text_field(&dict, "VERSION"),
        device_name: text_field(&dict, "DEVICE_NAME"),
    };
    if let Err(e) = record_enrollment(&kiwi, &device_id, enrolled).await {
        return e.into_response();
    }
    if let Err(e) = kiwi.db.delete_enrollment(&token) {
        tracing::warn!("could not delete an enrollment token: {e}");
    }
    let location = format!("{}/?enrolled=1", kiwi.public_url(&headers));
    let mut response = StatusCode::MOVED_PERMANENTLY.into_response();
    if let Ok(v) = HeaderValue::from_str(&location) {
        response.headers_mut().insert(header::LOCATION, v);
    }
    response
}

/// Device attributes from enrollment.
#[derive(Debug, Clone)]
pub struct Enrolled {
    pub udid: String,
    pub product: Option<String>,
    pub os_build: Option<String>,
    pub device_name: Option<String>,
}

/// Saves an enrolled UDID: approved at once for the owner, else pending (or kept approved
/// when the same UDID enrolls again), and asks the owner.
pub async fn record_enrollment(
    kiwi: &Arc<Kiwi>,
    device_id: &str,
    enrolled: Enrolled,
) -> ApiResult<()> {
    let device = kiwi
        .db
        .device(device_id)?
        .ok_or_else(|| ApiError::not_found("Device not found."))?;
    let now = Utc::now();
    let role = role(kiwi, &device);
    let previous = kiwi
        .db
        .registration(device_id)?
        .filter(|p| p.udid.eq_ignore_ascii_case(&enrolled.udid));
    let kept = previous
        .as_ref()
        .filter(|p| p.status == RegistrationStatus::Approved);
    let status = if role == Role::Owner || kept.is_some() {
        RegistrationStatus::Approved
    } else {
        RegistrationStatus::Pending
    };
    let row = RegistrationRow {
        device_id: device_id.to_owned(),
        udid: enrolled.udid.clone(),
        product: enrolled.product.clone(),
        os_build: enrolled.os_build.clone(),
        device_name: enrolled.device_name.clone(),
        status,
        requested_at: now,
        decided_at: match kept {
            Some(p) => p.decided_at,
            None => (status == RegistrationStatus::Approved).then_some(now),
        },
        apple_id: previous.as_ref().and_then(|p| p.apple_id.clone()),
        registered_at: previous.as_ref().and_then(|p| p.registered_at),
        error: None,
    };
    kiwi.db.save_registration(&row)?;
    tracing::info!(
        device = %device_id,
        status = row.status.as_str(),
        product = row.product.as_deref().unwrap_or("?"),
        "device enrolled"
    );
    let person = match &device.person_id {
        Some(id) => kiwi.db.person(id)?,
        None => None,
    };
    if device.name.is_none() {
        let generic = ["iPhone", "iPad", "iPod touch"];
        let name = enrolled
            .device_name
            .clone()
            .filter(|n| !generic.contains(&n.as_str()))
            .unwrap_or_else(|| {
                let kind = device_kind(enrolled.product.as_deref());
                match &person {
                    Some(p) => format!("{}'s {kind}", first_name(&p.name)),
                    None => kind.to_owned(),
                }
            });
        kiwi.db
            .set_device_name(device_id, Some(&name.chars().take(64).collect::<String>()))?;
    }
    if let Some(product) = &enrolled.product
        && device.model.is_none()
    {
        kiwi.db.set_device_model(device_id, product)?;
    }
    refresh_required(kiwi)?;
    match status {
        RegistrationStatus::Pending => {
            let device = kiwi.db.device(device_id)?.unwrap_or(device);
            if let Err(e) = ask_owner(kiwi, &device, person.as_ref(), &row).await {
                tracing::warn!("could not notify the owner: {e}");
            }
        }
        _ => start_apple_sync(kiwi),
    }
    Ok(())
}

/// "Register Noah's iPhone?" on the owner's devices.
async fn ask_owner(
    kiwi: &Kiwi,
    device: &DeviceRow,
    person: Option<&PersonRow>,
    reg: &RegistrationRow,
) -> ApiResult<()> {
    let who = person.map_or_else(|| device_name(device), |p| p.name.clone());
    let kind = device_kind(reg.product.as_deref());
    let title = match person {
        Some(p) => format!("Register {}'s {kind}?", first_name(&p.name)),
        None => format!("Register {}?", device_name(device)),
    };
    let key = format!("register:{}:{}", device.id, reg.requested_at.timestamp());
    for owner in owner_devices(kiwi)? {
        if kiwi.db.subscriptions(&owner.id)?.is_empty() {
            continue;
        }
        let badge = View::load(kiwi, &owner, &kiwi.default_public_url())?
            .updates()
            .len();
        let payload = Payload {
            title: title.clone(),
            body: format!("{who} · {kind}. Tap to choose apps and approve."),
            url: format!("/settings/family/devices/{}", device.id),
            badge,
            tag: format!("register:{}", device.id),
        };
        push::notify_once(kiwi, &owner.id, &format!("{key}:{}", owner.id), &payload).await?;
    }
    Ok(())
}

// ---- decisions ----

pub fn approve(kiwi: &Arc<Kiwi>, device_id: &str) -> ApiResult<()> {
    decide(kiwi, device_id, RegistrationStatus::Approved)?;
    start_apple_sync(kiwi);
    Ok(())
}

pub fn decline(kiwi: &Kiwi, device_id: &str) -> ApiResult<()> {
    decide(kiwi, device_id, RegistrationStatus::Declined)
}

fn decide(kiwi: &Kiwi, device_id: &str, status: RegistrationStatus) -> ApiResult<()> {
    if !kiwi.db.decide_registration(device_id, status, Utc::now())? {
        return Err(ApiError::not_found(
            "This device has not sent its ID to Kiwi yet.",
        ));
    }
    tracing::info!(device = %device_id, status = status.as_str(), "registration decided");
    refresh_required(kiwi)?;
    kiwi.kick.notify_one();
    Ok(())
}

pub fn forget_registration(kiwi: &Kiwi, device_id: &str) -> ApiResult<()> {
    kiwi.db.delete_registration(device_id)?;
    refresh_required(kiwi)
}

// ---- App Store Connect ----

pub fn load_credentials(path: &Path) -> Option<Credentials> {
    let bytes = match std::fs::read(path) {
        Ok(bytes) => bytes,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return None,
        Err(e) => {
            tracing::warn!("{}: {e}", path.display());
            return None;
        }
    };
    serde_json::from_slice(&bytes)
        .map_err(|e| tracing::warn!("{}: {e}", path.display()))
        .ok()
}

/// Writes the key readable only by the owner of the file.
fn save_credentials(path: &Path, credentials: &Credentials) -> std::io::Result<()> {
    use std::io::Write;
    let dir = path.parent().unwrap_or(Path::new("."));
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.as_file()
            .set_permissions(std::fs::Permissions::from_mode(0o600))?;
    }
    file.write_all(&serde_json::to_vec_pretty(credentials).map_err(std::io::Error::other)?)?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|e| e.error)?;
    Ok(())
}

fn apple_error(e: kiwi_appstore::Error) -> ApiError {
    match e {
        kiwi_appstore::Error::Key(m) => ApiError::bad_request(m),
        e if e.is_auth() => ApiError::bad_request(e.to_string()),
        e => ApiError::new(StatusCode::BAD_GATEWAY, "apple", e.to_string()),
    }
}

fn meta_time(kiwi: &Kiwi, key: &str) -> Option<Time> {
    kiwi.db
        .meta(key)
        .ok()
        .flatten()
        .and_then(|t| t.parse().ok())
}

fn record_apple(kiwi: &Kiwi, devices: Option<usize>, error: Option<&str>) {
    let now = Utc::now().to_rfc3339();
    let mut pairs = vec![("apple_error", error.unwrap_or("").to_owned())];
    if let Some(count) = devices {
        pairs.push(("apple_devices", count.to_string()));
        pairs.push(("apple_checked_at", now));
    }
    for (key, value) in pairs {
        if let Err(e) = kiwi.db.set_meta(key, &value) {
            tracing::warn!("could not save {key}: {e}");
        }
    }
}

pub fn apple_json(kiwi: &Kiwi) -> ApiResult<AppleJson> {
    let credentials = load_credentials(&kiwi.paths.appstore);
    let now = Utc::now();
    let profile = kiwi.signing_info().active(now).map(|plan| ProfileSummary {
        kiwi: is_kiwi_profile(kiwi, &plan.profile.path),
        uuid: plan.profile.uuid,
        name: plan.profile.name,
        devices: plan.profile.devices.len(),
        expires_at: plan.profile.expires_at,
    });
    Ok(AppleJson {
        connected: credentials.is_some(),
        issuer_id: credentials.as_ref().map(|c| c.issuer_id.clone()),
        key_id: credentials.as_ref().map(|c| c.key_id.clone()),
        devices: kiwi.db.meta("apple_devices")?.and_then(|n| n.parse().ok()),
        device_limit: kiwi_appstore::DEVICE_LIMIT,
        checked_at: meta_time(kiwi, "apple_checked_at"),
        error: kiwi.db.meta("apple_error")?.filter(|e| !e.is_empty()),
        profile,
    })
}

fn is_kiwi_profile(kiwi: &Kiwi, path: &Path) -> bool {
    path.starts_with(&kiwi.paths.profiles)
}

/// Saves an App Store Connect API key after checking it with Apple.
pub async fn connect(kiwi: &Arc<Kiwi>, credentials: Credentials) -> ApiResult<()> {
    let credentials = credentials.normalized().map_err(apple_error)?;
    let client = Client::new(&credentials, &kiwi.family.appstore_url).map_err(apple_error)?;
    let devices = client.devices().await.map_err(apple_error)?;
    save_credentials(&kiwi.paths.appstore, &credentials)
        .map_err(|e| ApiError::internal(format!("Could not save the key: {e}")))?;
    record_apple(
        kiwi,
        Some(devices.iter().filter(|d| d.is_ios()).count()),
        None,
    );
    tracing::info!(key = %credentials.key_id, "App Store Connect key saved");
    start_apple_sync(kiwi);
    Ok(())
}

pub fn disconnect(kiwi: &Kiwi) -> ApiResult<()> {
    match std::fs::remove_file(&kiwi.paths.appstore) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(ApiError::internal(e)),
    }
    for key in ["apple_error", "apple_devices", "apple_checked_at"] {
        kiwi.db.set_meta(key, "")?;
    }
    Ok(())
}

/// Registers approved devices and refreshes profiles in the background.
pub fn start_apple_sync(kiwi: &Arc<Kiwi>) {
    if !kiwi.paths.appstore.exists() {
        return;
    }
    let kiwi = kiwi.clone();
    let tracker = kiwi.jobs.tracker.clone();
    tracker.spawn(async move {
        if let Err(e) = sync_apple(&kiwi).await {
            tracing::warn!("registration with Apple failed: {e}");
        }
    });
}

/// Name for the device in the developer account.
fn apple_device_name(name: &str) -> String {
    let clean: String = name
        .chars()
        .filter(|c| !c.is_control())
        .take(APPLE_NAME_LIMIT)
        .collect();
    match clean.trim() {
        "" => "iPhone".into(),
        n => n.to_owned(),
    }
}

/// Registers approved devices in the developer account and, when the profiles Kiwi signs
/// with lack some of them, creates Kiwi's own development profiles that include every
/// enabled device. Then rescans and tells newly ready devices.
pub async fn sync_apple(kiwi: &Arc<Kiwi>) -> Result<(), String> {
    let _guard = kiwi.family.apple.lock().await;
    *kiwi.family.apple_attempt.lock().unwrap() = Some(Instant::now());
    let Some(credentials) = load_credentials(&kiwi.paths.appstore) else {
        return Ok(());
    };
    let result = sync_with(kiwi, &credentials).await;
    match &result {
        Ok(count) => record_apple(kiwi, Some(*count), None),
        Err(e) => record_apple(kiwi, None, Some(e)),
    }
    kiwi.rescan_signing().await;
    if let Err(e) = notify_ready(kiwi).await {
        tracing::warn!("could not notify ready devices: {e}");
    }
    kiwi.kick.notify_one();
    result.map(drop)
}

/// Returns the number of iOS devices in the account.
async fn sync_with(kiwi: &Arc<Kiwi>, credentials: &Credentials) -> Result<usize, String> {
    let client = Client::new(credentials, &kiwi.family.appstore_url).map_err(|e| e.to_string())?;
    let mut account = client.devices().await.map_err(|e| e.to_string())?;
    let approved: Vec<RegistrationRow> = kiwi
        .db
        .registrations()
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|r| r.status == RegistrationStatus::Approved)
        .collect();
    let now = Utc::now();
    for reg in &approved {
        let existing = account.iter().find(|d| d.has_udid(&reg.udid)).cloned();
        let result = match existing {
            Some(d) if d.enabled() => Ok(d),
            Some(d) => client.enable_device(&d.id).await,
            None => {
                let name = kiwi
                    .db
                    .device(&reg.device_id)
                    .ok()
                    .flatten()
                    .map_or_else(|| "iPhone".into(), |d| device_name(&d));
                client
                    .register_device(&apple_device_name(&name), &reg.udid)
                    .await
            }
        };
        match result {
            Ok(device) => {
                tracing::info!(device = %reg.device_id, "registered with Apple");
                kiwi.db
                    .set_registration_apple(&reg.device_id, &reg.udid, &device.id, now)
                    .map_err(|e| e.to_string())?;
                account.retain(|d| d.id != device.id);
                account.push(device);
            }
            Err(e) => {
                let message = e.to_string();
                kiwi.db
                    .set_registration_error(&reg.device_id, Some(&message))
                    .map_err(|e| e.to_string())?;
                if e.is_auth() {
                    return Err(message);
                }
            }
        }
    }
    let registered: Vec<String> = approved
        .iter()
        .filter(|r| account.iter().any(|d| d.has_udid(&r.udid) && d.enabled()))
        .map(|r| r.udid.clone())
        .collect();
    let ios = account.iter().filter(|d| d.is_ios()).count();
    let signing = kiwi.signing_info();
    let wildcard_ok = signing
        .active(now)
        .is_some_and(|plan| registered.iter().all(|u| plan.profile.covers(u)));
    let explicit = explicit_to_copy(kiwi, &signing, &registered, now);
    if registered.is_empty() || (wildcard_ok && explicit.is_empty()) {
        return Ok(ios);
    }
    let sha1s: HashSet<String> = signing
        .identities
        .iter()
        .map(|i| i.sha1.to_uppercase())
        .collect();
    let certificates: Vec<String> = client
        .development_certificates()
        .await
        .map_err(|e| e.to_string())?
        .into_iter()
        .filter(|c| c.sha1.as_ref().is_some_and(|s| sha1s.contains(s)))
        .map(|c| c.id)
        .collect();
    if certificates.is_empty() {
        return Err(
            "The Apple Development certificate in the Mac's keychain is not in this developer account.".into(),
        );
    }
    let device_ids: Vec<String> = account
        .iter()
        .filter(|d| d.is_ios() && d.enabled())
        .map(|d| d.id.clone())
        .collect();
    let stamp = now.format("%Y-%m-%d %H.%M").to_string();
    let mut identifiers = Vec::new();
    if !wildcard_ok {
        identifiers.push("*".to_owned());
    }
    identifiers.extend(explicit);
    let mut first_error = None;
    for identifier in identifiers {
        let created = async {
            let bundle = match client.bundle_id(&identifier).await? {
                Some(b) => b,
                None if identifier == "*" => client.create_bundle_id("*", "Kiwi Wildcard").await?,
                None => {
                    return Err(kiwi_appstore::Error::Response(format!(
                        "no App ID {identifier}"
                    )));
                }
            };
            let name = format!("Kiwi {identifier} {stamp}");
            client
                .create_profile(&name, &bundle.id, &certificates, &device_ids)
                .await
        }
        .await;
        match created.map_err(|e| e.to_string()).and_then(|p| {
            install_profile(&kiwi.paths.profiles, &p.content).map_err(|e| e.to_string())
        }) {
            Ok(path) => tracing::info!("created profile {} for {identifier}", path.display()),
            Err(e) => {
                tracing::warn!("could not create a profile for {identifier}: {e}");
                first_error.get_or_insert(format!("Creating a profile for {identifier}: {e}"));
            }
        }
    }
    match first_error {
        Some(e) => Err(e),
        None => Ok(ios),
    }
}

/// Explicit App IDs whose profiles Kiwi would sign catalog apps with but that lack some of
/// the registered devices.
fn explicit_to_copy(
    kiwi: &Kiwi,
    signing: &SigningInfo,
    registered: &[String],
    now: Time,
) -> Vec<String> {
    let Some(team) = kiwi.team_id() else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for app in kiwi.catalog.apps(None, None) {
        let signed = kiwi_signer::signed_bundle_id(&app.app().bundle_id, &team);
        let Ok(plan) = signing.plan(&signed, now) else {
            continue;
        };
        if plan.profile.is_wildcard() || registered.iter().all(|u| plan.profile.covers(u)) {
            continue;
        }
        if let Some((_, id)) = plan.profile.app_id_pattern.split_once('.')
            && !out.iter().any(|o| o == id)
        {
            out.push(id.to_owned());
        }
    }
    out
}

/// Saves a new profile in Kiwi's profile directory, replacing Kiwi's older ones for the
/// same App ID.
pub fn install_profile(dir: &Path, content: &[u8]) -> Result<PathBuf, String> {
    let profile = kiwi_signer::parse_profile(content).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let uuid: String = profile
        .uuid
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    if uuid.is_empty() {
        return Err("the profile has no UUID".into());
    }
    let path = dir.join(format!("{uuid}.mobileprovision"));
    let partial = dir.join(format!("{uuid}.partial"));
    std::fs::write(&partial, content).map_err(|e| format!("{}: {e}", partial.display()))?;
    std::fs::rename(&partial, &path).map_err(|e| format!("{}: {e}", path.display()))?;
    for (old, parsed) in kiwi_signer::load_profiles(&[dir.to_path_buf()]) {
        if old != path
            && parsed.is_ok_and(|p| p.app_id_pattern == profile.app_id_pattern)
            && let Err(e) = std::fs::remove_file(&old)
        {
            tracing::warn!("could not remove {}: {e}", old.display());
        }
    }
    Ok(path)
}

/// "This iPhone is ready" for members whose approved device is now in the profile.
pub async fn notify_ready(kiwi: &Kiwi) -> ApiResult<()> {
    let signing = kiwi.signing_info();
    let now = Utc::now();
    for reg in kiwi.db.registrations()? {
        if reg.status != RegistrationStatus::Approved || !in_profile(&signing, &reg.udid, now) {
            continue;
        }
        let Some(device) = kiwi.db.device(&reg.device_id)? else {
            continue;
        };
        if role(kiwi, &device) == Role::Owner || kiwi.db.subscriptions(&device.id)?.is_empty() {
            continue;
        }
        let key = format!("ready:{}:{}", device.id, reg.udid);
        let payload = Payload {
            title: "Kiwi".into(),
            body: format!(
                "This {} is ready. Open Kiwi to install apps.",
                device_kind(reg.product.as_deref())
            ),
            url: "/".into(),
            badge: 0,
            tag: "registered".into(),
        };
        push::notify_once(kiwi, &device.id, &key, &payload).await?;
    }
    Ok(())
}

/// Scheduler work: the owner while unknown, ready notifications, retrying registration
/// with Apple and old enrollment tokens.
pub async fn tick(kiwi: &Arc<Kiwi>) {
    if kiwi.family.owner().is_none() {
        refresh_owner(kiwi, false).await;
    }
    if let Err(e) = notify_ready(kiwi).await {
        tracing::warn!("ready notifications failed: {e}");
    }
    let due = kiwi
        .family
        .apple_attempt
        .lock()
        .unwrap()
        .is_none_or(|at| at.elapsed() >= APPLE_RETRY);
    if due && kiwi.paths.appstore.exists() && waiting_for_apple(kiwi).unwrap_or(false) {
        start_apple_sync(kiwi);
    }
    if let Err(e) = kiwi.db.prune_enrollments(Utc::now() - ENROLLMENT_TTL) {
        tracing::warn!("could not prune enrollments: {e}");
    }
}

/// Approved devices that are not in the profile yet.
fn waiting_for_apple(kiwi: &Kiwi) -> ApiResult<bool> {
    let signing = kiwi.signing_info();
    let now = Utc::now();
    Ok(kiwi
        .db
        .registrations()?
        .iter()
        .any(|r| r.status == RegistrationStatus::Approved && !in_profile(&signing, &r.udid, now)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn udids() {
        assert!(valid_udid("00008110-001A2B3C4D5E801E"));
        assert!(valid_udid("0123456789abcdef0123456789abcdef01234567"));
        assert!(!valid_udid("00008110-001A2B3C4D5E801"));
        assert!(!valid_udid("not-a-udid"));
        assert!(!valid_udid(""));
    }

    #[test]
    fn names() {
        assert_eq!(login_name("noah@example.com"), "noah");
        assert_eq!(first_name("Alex Morgan"), "Alex");
        assert_eq!(device_kind(Some("iPad14,1")), "iPad");
        assert_eq!(device_kind(None), "iPhone");
        assert_eq!(apple_device_name(&"x".repeat(80)).len(), 50);
        assert_eq!(apple_device_name(" \n "), "iPhone");
    }

    #[test]
    fn enrollment_profile() {
        let bytes = mobileconfig("https://kiwi.test", "TOKEN");
        let value: plist::Value = plist::from_bytes(&bytes).unwrap();
        let root = value.as_dictionary().unwrap();
        assert_eq!(
            root.get("PayloadType").and_then(plist::Value::as_string),
            Some("Profile Service")
        );
        let content = root
            .get("PayloadContent")
            .and_then(plist::Value::as_dictionary)
            .unwrap();
        assert_eq!(
            content.get("URL").and_then(plist::Value::as_string),
            Some("https://kiwi.test/enroll/TOKEN")
        );
        assert_eq!(
            content.get("Challenge").and_then(plist::Value::as_string),
            Some("TOKEN")
        );
        let uuid = root
            .get("PayloadUUID")
            .and_then(plist::Value::as_string)
            .unwrap();
        assert_eq!(uuid.len(), 36);
        assert_eq!(&uuid[14..15], "4");
        let description = root
            .get("PayloadDescription")
            .and_then(plist::Value::as_string)
            .unwrap();
        assert!(description.contains("Kiwi at kiwi.test"));
    }

    fn person(new_apps: bool, since: Option<Time>) -> PersonRow {
        let now = Utc::now();
        PersonRow {
            id: "noah@example.com".into(),
            name: "Noah".into(),
            renamed: false,
            new_apps,
            new_apps_since: since,
            created_at: now,
            last_seen: now,
        }
    }

    #[test]
    fn explicit_choices_and_apps_added_later() {
        let now = Utc::now();
        let prefs = |first_seen| AppPrefs {
            first_seen,
            shown: None,
            original: Default::default(),
        };
        let old = prefs(now - chrono::Duration::days(2));
        let new = prefs(now);
        let mut choices = HashMap::new();
        choices.insert(("s".to_owned(), "chosen".to_owned()), true);
        choices.insert(("s".to_owned(), "off".to_owned()), false);
        let off = person(false, None);
        assert_eq!(
            allowed(&off, &choices, "s", "chosen", Some(&old)),
            (true, true)
        );
        assert_eq!(
            allowed(&off, &choices, "s", "off", Some(&new)),
            (false, true)
        );
        assert_eq!(
            allowed(&off, &choices, "s", "other", Some(&new)),
            (false, false)
        );
        let on = person(true, Some(now - chrono::Duration::days(1)));
        // Only apps first seen after the switch was turned on.
        assert_eq!(
            allowed(&on, &choices, "s", "other", Some(&new)),
            (true, false)
        );
        assert_eq!(
            allowed(&on, &choices, "s", "other", Some(&old)),
            (false, false)
        );
        assert_eq!(
            allowed(&on, &choices, "s", "off", Some(&new)),
            (false, true)
        );
    }
}
