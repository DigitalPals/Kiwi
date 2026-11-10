//! Device identification (Tailscale whois or cookie), OS versions, devicectl links and sync.

use std::{
    collections::HashMap,
    future::Future,
    net::{IpAddr, SocketAddr},
    pin::Pin,
    sync::{Arc, Mutex, Weak, atomic::Ordering},
    time::{Duration, Instant},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use chrono::Utc;
use kiwi_device::{CoreDevice, DeviceApp, Tailscale, TailscalePeer, TailscaleUser, UaSource};
use kiwi_source::DeviceProfile;
use serde::Serialize;

use crate::{
    Kiwi,
    catalog::CatalogApp,
    db::{Db, DeviceRow, InstallRow, Platform, Time},
    error::{ApiError, ApiResult},
};

pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Platform tools, behind a trait so tests can inject fakes.
pub trait DeviceTools: Send + Sync + 'static {
    fn whois(&self, ip: IpAddr) -> BoxFuture<'_, Result<Option<TailscalePeer>, String>>;
    /// The Tailscale user who owns the Mac; `None` for a tagged machine.
    fn owner(&self) -> BoxFuture<'_, Result<Option<TailscaleUser>, String>>;
    fn devicectl_available(&self) -> BoxFuture<'_, bool>;
    fn list_devices(&self) -> BoxFuture<'_, Result<Vec<CoreDevice>, String>>;
    fn list_apps<'a>(&'a self, device: &'a str) -> BoxFuture<'a, Result<Vec<DeviceApp>, String>>;
}

/// The real tailscaled (or `tailscale` CLI) and `xcrun devicectl`.
pub struct SystemDevices {
    pub tailscale: Tailscale,
}

impl DeviceTools for SystemDevices {
    fn whois(&self, ip: IpAddr) -> BoxFuture<'_, Result<Option<TailscalePeer>, String>> {
        Box::pin(async move { self.tailscale.whois(ip).await.map_err(|e| e.to_string()) })
    }
    fn owner(&self) -> BoxFuture<'_, Result<Option<TailscaleUser>, String>> {
        Box::pin(async move { self.tailscale.owner().await.map_err(|e| e.to_string()) })
    }
    fn devicectl_available(&self) -> BoxFuture<'_, bool> {
        Box::pin(kiwi_device::devicectl_available())
    }
    fn list_devices(&self) -> BoxFuture<'_, Result<Vec<CoreDevice>, String>> {
        Box::pin(async { kiwi_device::list_devices().await.map_err(|e| e.to_string()) })
    }
    fn list_apps<'a>(&'a self, device: &'a str) -> BoxFuture<'a, Result<Vec<DeviceApp>, String>> {
        Box::pin(async move {
            kiwi_device::list_apps(device)
                .await
                .map_err(|e| e.to_string())
        })
    }
}

pub const COOKIE: &str = "kiwi_device";
const WHOIS_TTL: Duration = Duration::from_secs(10 * 60);
/// A failed whois is not repeated for this long, so a stopped tailscaled does not cost two
/// lookups per request.
const WHOIS_ERROR_TTL: Duration = Duration::from_secs(15);
const CORE_DEVICES_TTL: Duration = Duration::from_secs(60);
/// devicectl may not list a Kiwi install completed less than this long ago yet, or still
/// list the version it replaces: such installs are neither marked removed nor given the
/// listed version.
const INSTALL_GRACE: chrono::Duration = chrono::Duration::minutes(10);

#[derive(Default)]
pub struct DeviceCaches {
    whois: Mutex<HashMap<IpAddr, (Instant, Option<TailscalePeer>)>>,
    whois_failed: Mutex<HashMap<IpAddr, Instant>>,
    /// Only requests for the same address wait for each other. Weak entries do not keep
    /// finished lookups alive, including ones whose request was cancelled.
    whois_queries: Mutex<HashMap<IpAddr, Weak<tokio::sync::Mutex<()>>>>,
    core: Mutex<Option<(Instant, Vec<CoreDevice>)>>,
    core_query: tokio::sync::Mutex<()>,
}

impl DeviceCaches {
    fn cached_whois(&self, ip: IpAddr) -> Option<ApiResult<Option<TailscalePeer>>> {
        if let Some((at, peer)) = self.whois.lock().unwrap().get(&ip)
            && at.elapsed() < WHOIS_TTL
        {
            return Some(Ok(peer.clone()));
        }
        self.whois_failed
            .lock()
            .unwrap()
            .get(&ip)
            .is_some_and(|at| at.elapsed() < WHOIS_ERROR_TTL)
            .then(|| Err(ApiError::device_unidentified()))
    }

    async fn whois(&self, tools: &dyn DeviceTools, ip: IpAddr) -> ApiResult<Option<TailscalePeer>> {
        if let Some(cached) = self.cached_whois(ip) {
            return cached;
        }
        let query = {
            let mut queries = self.whois_queries.lock().unwrap();
            queries.retain(|_, query| query.strong_count() > 0);
            match queries.get(&ip).and_then(Weak::upgrade) {
                Some(query) => query,
                None => {
                    let query = Arc::new(tokio::sync::Mutex::new(()));
                    queries.insert(ip, Arc::downgrade(&query));
                    query
                }
            }
        };
        let _guard = query.lock().await;
        // A concurrent request may have filled either cache while we waited.
        if let Some(cached) = self.cached_whois(ip) {
            return cached;
        }
        for _ in 0..2 {
            match tools.whois(ip).await {
                Ok(peer) => {
                    self.whois_failed.lock().unwrap().remove(&ip);
                    let mut peers = self.whois.lock().unwrap();
                    peers.retain(|_, (at, _)| at.elapsed() < WHOIS_TTL);
                    peers.insert(ip, (Instant::now(), peer.clone()));
                    return Ok(peer);
                }
                Err(e) => tracing::warn!("tailscale whois {ip} failed: {e}"),
            }
        }
        let mut failed = self.whois_failed.lock().unwrap();
        failed.retain(|_, at| at.elapsed() < WHOIS_ERROR_TTL);
        failed.insert(ip, Instant::now());
        Err(ApiError::device_unidentified())
    }

    async fn core_devices(&self, tools: &dyn DeviceTools, force: bool) -> Vec<CoreDevice> {
        let requested = Instant::now();
        let cached = || {
            self.core
                .lock()
                .unwrap()
                .as_ref()
                .filter(|(at, _)| at.elapsed() < CORE_DEVICES_TTL && (!force || *at >= requested))
                .map(|(_, list)| list.clone())
        };
        // Warm reads stay immediate even during a forced refresh.
        if let Some(list) = cached() {
            return list;
        }
        let _query = self.core_query.lock().await;
        // Even forced requests can share a refresh completed since they started waiting.
        if let Some(list) = cached() {
            return list;
        }
        match tools.list_devices().await {
            Ok(list) => {
                *self.core.lock().unwrap() = Some((Instant::now(), list.clone()));
                list
            }
            Err(e) => {
                tracing::warn!("devicectl list devices failed: {e}");
                self.core
                    .lock()
                    .unwrap()
                    .as_ref()
                    .map(|(_, list)| list.clone())
                    .unwrap_or_default()
            }
        }
    }
}

/// The client address when the request was proxied by `tailscale serve` on loopback: the
/// rightmost `X-Forwarded-For` entry (the one the proxy added).
pub fn forwarded_client(peer: Option<SocketAddr>, forwarded_for: Option<&str>) -> Option<IpAddr> {
    if !peer?.ip().to_canonical().is_loopback() {
        return None;
    }
    forwarded_for?
        .rsplit(',')
        .next()?
        .trim()
        .parse::<IpAddr>()
        .ok()
        .map(|ip| ip.to_canonical())
}

pub fn cookie_device_id(cookie_header: Option<&str>) -> Option<String> {
    cookie_header?
        .split(';')
        .filter_map(|c| c.trim().split_once('='))
        .find(|(k, _)| *k == COOKIE)
        .map(|(_, v)| v.trim().to_owned())
        .filter(|v| valid_local_id(v))
}

fn valid_local_id(id: &str) -> bool {
    id.strip_prefix("local:").is_some_and(|rest| {
        rest.len() == 22
            && rest
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
    })
}

/// 128 random bits, base64url (22 characters).
pub fn random_token() -> String {
    let mut bytes = [0u8; 16];
    getrandom::fill(&mut bytes).expect("system random number generator");
    B64.encode(bytes)
}

/// The `os_version_source` priority; `Db::observe_device_os` repeats it in SQL.
fn priority(source: Option<&str>) -> u8 {
    match source {
        Some("devicectl") => 5,
        Some("installer") => 4,
        Some("manual") => 3,
        Some("android_app") => 2,
        Some("safari" | "android") => 1,
        _ => 0,
    }
}

/// Records an OS version unless a higher-priority source already provided one.
pub fn observe_os(device: &mut DeviceRow, version: &str, source: &str) -> bool {
    if priority(Some(source)) < priority(device.os_version_source.as_deref())
        || (device.os_version.as_deref() == Some(version)
            && device.os_version_source.as_deref() == Some(source))
    {
        return false;
    }
    device.os_version = Some(version.to_owned());
    device.os_version_source = Some(source.to_owned());
    true
}

/// Where a device's platform comes from (`Device.platform_source` in api.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum PlatformSource {
    /// The owner chose it.
    Chosen,
    /// Tailscale or a User-Agent reported it.
    Detected,
    /// Nothing told: Kiwi assumes iOS.
    Assumed,
}

/// The platform a device installs for (architecture.md "Device platforms"): the owner's
/// choice, else the one Tailscale or a User-Agent reported, else iOS.
pub fn platform(device: &DeviceRow) -> (Platform, PlatformSource) {
    match (device.platform, device.detected_platform) {
        (Some(chosen), _) => (chosen, PlatformSource::Chosen),
        (None, Some(detected)) => (detected, PlatformSource::Detected),
        (None, None) => (Platform::Ios, PlatformSource::Assumed),
    }
}

/// The platform a device installs for (see [`platform`]).
pub fn effective_platform(device: &DeviceRow) -> Platform {
    platform(device).0
}

/// Which program sent a request (`Device.client` in api.md).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Client {
    /// The web app, in a browser or as a Home Screen app.
    #[default]
    Web,
    /// Kiwi's Android app.
    AndroidApp,
}

impl Client {
    pub fn of(user_agent: Option<&str>) -> Self {
        match user_agent.is_some_and(kiwi_device::is_native_android) {
            true => Self::AndroidApp,
            false => Self::Web,
        }
    }
}

/// The platform of a Tailscale node's OS (`iOS` for iPhones and iPads, `android`).
fn tailscale_platform(os: &str) -> Option<Platform> {
    match os.to_ascii_lowercase().as_str() {
        "ios" | "ipados" => Some(Platform::Ios),
        "android" => Some(Platform::Android),
        _ => None,
    }
}

/// Records the platform Tailscale or a User-Agent reported, unless one was recorded
/// before: the first report wins, so an iPhone never turns into an Android device.
fn observe_platform(device: &mut DeviceRow, platform: Option<Platform>) -> bool {
    match platform {
        Some(platform) if device.detected_platform.is_none() => {
            device.detected_platform = Some(platform);
            true
        }
        _ => false,
    }
}

/// Takes the platform, OS version and model from a Safari, OTA installer, Android browser
/// or Kiwi Android app User-Agent.
pub fn observe_user_agent(device: &mut DeviceRow, user_agent: Option<&str>) -> bool {
    let Some(user_agent) = user_agent else {
        return false;
    };
    let reported = kiwi_device::user_agent_platform(user_agent).and_then(Platform::parse);
    let mut changed = observe_platform(device, reported);
    let Some(info) = kiwi_device::parse_user_agent(user_agent) else {
        return changed;
    };
    // A client of the other platform (a desktop-mode or spoofed User-Agent) tells nothing
    // about this device.
    if reported != device.detected_platform {
        return changed;
    }
    if let Some(version) = &info.os_version {
        changed |= observe_os(device, version, info.source.as_str());
    }
    // An installer names the exact hardware. An Android client only tells the platform,
    // which never replaces a model Apple's tools reported.
    let trusted = match info.source {
        UaSource::Installer => true,
        UaSource::Android | UaSource::AndroidApp => device.model.is_none(),
        UaSource::Safari => false,
    };
    if trusted
        && let Some(model) = info.model
        && device.model.as_ref() != Some(&model)
    {
        device.model = Some(model);
        changed = true;
    }
    changed
}

/// [`observe_user_agent`], also writing the OS version and model (only those columns) to
/// the database.
pub fn record_user_agent(
    db: &Db,
    device: &mut DeviceRow,
    user_agent: Option<&str>,
) -> rusqlite::Result<()> {
    let before = device.clone();
    if !observe_user_agent(device, user_agent) {
        return Ok(());
    }
    if let Some(platform) = device.detected_platform
        && before.detected_platform.is_none()
    {
        db.set_detected_platform(&device.id, platform)?;
    }
    if let (Some(version), Some(source)) = (&device.os_version, &device.os_version_source)
        && (device.os_version != before.os_version
            || device.os_version_source != before.os_version_source)
    {
        db.observe_device_os(&device.id, version, source)?;
    }
    if let Some(model) = &device.model
        && device.model != before.model
    {
        db.set_device_model(&device.id, model)?;
    }
    Ok(())
}

/// What the device can install: releases for its platform (see [`platform`]) and, once
/// its OS version is known, for that version. Without a version (an empty one) every
/// release of the platform is offered.
pub fn device_profile(device: &DeviceRow) -> DeviceProfile {
    let os_version = device.os_version.clone().unwrap_or_default();
    match effective_platform(device) {
        // Android browsers rarely report the OS version, so Android releases declare no
        // minimum; the platform alone decides.
        Platform::Android => DeviceProfile {
            os_version,
            platform: "android".into(),
            family: 1,
        },
        Platform::Ios => {
            let ipad = device
                .model
                .as_deref()
                .is_some_and(|m| m.starts_with("iPad"));
            DeviceProfile {
                os_version,
                platform: "ios".into(),
                family: if ipad { 2 } else { 1 },
            }
        }
    }
}

/// What identified a request.
#[derive(Debug, Clone, Default)]
pub struct Requester {
    pub peer: Option<SocketAddr>,
    pub forwarded_for: Option<String>,
    pub cookie: Option<String>,
    pub user_agent: Option<String>,
}

/// The device of a request, and a `kiwi_device` cookie value to set when it is new.
pub async fn resolve(kiwi: &Kiwi, req: &Requester) -> ApiResult<(DeviceRow, Option<String>)> {
    let now = Utc::now();
    let peer = match forwarded_client(req.peer, req.forwarded_for.as_deref()) {
        Some(ip) => whois(kiwi, ip).await?,
        None => None,
    };
    let mut set_cookie = None;
    let (id, kind) = match &peer {
        Some(peer) => (format!("ts:{}", peer.stable_id), "tailscale"),
        None => match cookie_device_id(req.cookie.as_deref()) {
            Some(id) => (id, "browser"),
            None => {
                let id = format!("local:{}", random_token());
                set_cookie = Some(id.clone());
                (id, "browser")
            }
        },
    };
    let user = peer.as_ref().map(user_of);
    // Tailscale knows a peer's OS even when its browser asks for desktop sites.
    let peer_platform = peer
        .as_ref()
        .and_then(|p| p.os.as_deref())
        .and_then(tailscale_platform);
    let mut device = match kiwi.db.device(&id)? {
        Some(d) => d,
        None => {
            let mut d = DeviceRow::new(id.clone(), kind, now);
            d.tailscale_name = peer.as_ref().map(|p| p.name.clone());
            d.person_id = user.clone();
            kiwi.db.insert_device(&d)?;
            kiwi.db.device(&id)?.unwrap_or(d)
        }
    };
    let renamed = peer
        .as_ref()
        .is_some_and(|p| device.tailscale_name.as_ref() != Some(&p.name));
    if let Some(user) = user
        && device.person_id.as_ref() != Some(&user)
    {
        kiwi.db.set_device_person(&id, &user)?;
        device.person_id = Some(user);
    }
    if renamed || now - device.last_seen > chrono::Duration::minutes(1) {
        let name = peer.map(|p| p.name);
        kiwi.db.touch_device(&id, now, name.as_deref())?;
        device.last_seen = now;
        if name.is_some() {
            device.tailscale_name = name;
        }
    }
    if observe_platform(&mut device, peer_platform)
        && let Some(platform) = device.detected_platform
    {
        kiwi.db.set_detected_platform(&id, platform)?;
    }
    if device.detected_platform == Some(Platform::Android) && device.model.is_none() {
        kiwi.db.set_device_model(&id, kiwi_device::ANDROID_MODEL)?;
        device.model = Some(kiwi_device::ANDROID_MODEL.into());
    }
    record_user_agent(&kiwi.db, &mut device, req.user_agent.as_deref())?;
    Ok((device, set_cookie))
}

/// The Tailscale user of a peer: their login, or `node:<StableID>` for a node without a
/// user (which is never the owner).
fn user_of(peer: &TailscalePeer) -> String {
    match &peer.user_login {
        Some(login) => login.clone(),
        None => format!("node:{}", peer.stable_id),
    }
}

/// The Tailscale peer of a forwarded client IP; `None` when it is not a peer. Lookup errors
/// are retried once, then reported (and repeated for [`WHOIS_ERROR_TTL`] without another
/// lookup), so the phone is not mistaken for a new cookie device.
async fn whois(kiwi: &Kiwi, ip: IpAddr) -> ApiResult<Option<TailscalePeer>> {
    kiwi.caches.whois(kiwi.devices.as_ref(), ip).await
}

/// CoreDevices from devicectl, cached for a minute (or refreshed when `force`).
pub async fn core_devices(kiwi: &Kiwi, force: bool) -> Vec<CoreDevice> {
    if !kiwi.devicectl_available.load(Ordering::Relaxed) {
        return Vec::new();
    }
    kiwi.caches.core_devices(kiwi.devices.as_ref(), force).await
}

// ---- JSON ----

#[derive(Debug, Serialize)]
pub struct DeviceJson {
    pub id: String,
    pub name: String,
    pub kind: String,
    pub tailscale_name: Option<String>,
    pub os_version: Option<String>,
    pub os_version_source: Option<String>,
    pub model: Option<String>,
    /// What the device installs (see [`platform`]).
    pub platform: Platform,
    pub platform_source: PlatformSource,
    /// What Kiwi would take without the owner's choice; `None` when nothing told.
    pub detected_platform: Option<Platform>,
    /// The program that sent this request; other devices' are `web`.
    pub client: Client,
    pub devicectl: Option<LinkedJson>,
    pub last_seen: Time,
}

#[derive(Debug, Serialize)]
pub struct LinkedJson {
    pub id: String,
    pub name: String,
    pub reachable: bool,
    pub last_sync_at: Option<Time>,
    pub last_error: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct CoreDeviceJson {
    pub id: String,
    pub name: String,
    pub udid: Option<String>,
    pub model: Option<String>,
    pub platform: Option<String>,
    pub os_version: Option<String>,
    pub physical: bool,
    pub pairing_state: Option<String>,
    pub tunnel_state: Option<String>,
    pub transport: Option<String>,
    pub linked_device_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct DevicesJson {
    pub devices: Vec<DeviceJson>,
    pub core_devices: Vec<CoreDeviceJson>,
}

/// A device as the API shows it; `client` is the program of the request (`Web` for other
/// devices than the requesting one).
pub fn device_json(d: &DeviceRow, client: Client) -> DeviceJson {
    let fallback = if d.kind == "tailscale" {
        "Device"
    } else {
        "Browser"
    };
    let (platform, platform_source) = platform(d);
    DeviceJson {
        id: d.id.clone(),
        name: d
            .name
            .clone()
            .or_else(|| d.tailscale_name.clone())
            .unwrap_or_else(|| fallback.into()),
        kind: d.kind.clone(),
        tailscale_name: d.tailscale_name.clone(),
        os_version: d.os_version.clone(),
        os_version_source: d.os_version_source.clone(),
        model: d.model.clone(),
        platform,
        platform_source,
        detected_platform: d.detected_platform,
        client,
        devicectl: d.devicectl_id.as_ref().map(|id| LinkedJson {
            id: id.clone(),
            name: d.devicectl_name.clone().unwrap_or_else(|| id.clone()),
            reachable: d.devicectl_reachable,
            last_sync_at: d.last_sync_at,
            last_error: d.last_sync_error.clone(),
        }),
        last_seen: d.last_seen,
    }
}

/// Every device and CoreDevice; `current` is the requesting device and its client.
pub async fn devices_json(
    kiwi: &Kiwi,
    force: bool,
    current: (&str, Client),
) -> ApiResult<DevicesJson> {
    let devices = kiwi.db.devices()?;
    let cores = core_devices(kiwi, force).await;
    Ok(DevicesJson {
        core_devices: cores
            .into_iter()
            .map(|c| CoreDeviceJson {
                linked_device_id: devices
                    .iter()
                    .find(|d| d.devicectl_id.as_ref() == Some(&c.identifier))
                    .map(|d| d.id.clone()),
                id: c.identifier,
                name: c.name,
                udid: c.udid,
                model: c.model,
                platform: c.platform,
                os_version: c.os_version,
                physical: c.physical,
                pairing_state: c.pairing_state,
                tunnel_state: c.tunnel_state,
                transport: c.transport,
            })
            .collect(),
        devices: devices
            .iter()
            .map(|d| device_json(d, client_of(d, current)))
            .collect(),
    })
}

/// The client of a device in a list: the request's for the requesting device.
pub fn client_of(device: &DeviceRow, (id, client): (&str, Client)) -> Client {
    if device.id == id { client } else { Client::Web }
}

// ---- linking and sync ----

fn linkable(core: &CoreDevice, allow_simulators: bool) -> bool {
    core.physical || allow_simulators
}

fn link_columns(core: &CoreDevice) -> (&str, &str, bool, Option<&str>) {
    (
        &core.identifier,
        &core.name,
        core.reachable(),
        core.udid.as_deref(),
    )
}

/// Links a Kiwi device to a CoreDevice (or unlinks it with `None`) and returns the
/// CoreDevice. A CoreDevice is linked to at most one Kiwi device.
pub async fn link(
    kiwi: &Kiwi,
    device_id: &str,
    core_id: Option<&str>,
) -> ApiResult<Option<CoreDevice>> {
    let Some(core_id) = core_id else {
        kiwi.db.set_device_link(device_id, None)?;
        return Ok(None);
    };
    let mut cores = core_devices(kiwi, false).await;
    if !cores.iter().any(|c| c.identifier == core_id) {
        cores = core_devices(kiwi, true).await;
    }
    let core = cores
        .into_iter()
        .find(|c| c.identifier == core_id)
        .ok_or_else(|| {
            ApiError::bad_request("That device is not known to devicectl on the Mac.")
        })?;
    if !linkable(&core, kiwi.config.allow_simulators) {
        return Err(ApiError::bad_request(
            "Simulators can't be linked (start Kiwi with --allow-simulators for testing).",
        ));
    }
    for other in kiwi.db.devices()? {
        if other.id != device_id && other.devicectl_id.as_deref() == Some(core_id) {
            kiwi.db.set_device_link(&other.id, None)?;
        }
    }
    kiwi.db
        .set_device_link(device_id, Some(link_columns(&core)))?;
    Ok(Some(core))
}

/// Links automatically when exactly one linkable paired CoreDevice and one Kiwi device exist.
pub fn auto_link_candidate<'a>(
    cores: &'a [CoreDevice],
    devices: &[DeviceRow],
    allow_simulators: bool,
) -> Option<&'a CoreDevice> {
    let candidates: Vec<&CoreDevice> = cores
        .iter()
        .filter(|c| linkable(c, allow_simulators) && c.paired())
        .collect();
    match (candidates.as_slice(), devices) {
        ([core], [device]) if device.devicectl_id.is_none() => Some(core),
        _ => None,
    }
}

/// Reconciles a device's installs with the apps devicectl reports. Returns rows to save.
pub fn reconcile(
    device_id: &str,
    installs: &[InstallRow],
    apps: &[DeviceApp],
    catalog: &[CatalogApp],
    team_id: Option<&str>,
    now: Time,
) -> Vec<InstallRow> {
    let mut changed = Vec::new();
    for install in installs {
        let mut row = install.clone();
        match apps
            .iter()
            .find(|a| a.bundle_id == install.signed_bundle_id)
        {
            Some(app) => {
                row.last_seen_at = Some(now);
                row.removed_at = None;
                let other_version = app.version.as_ref().filter(|v| **v != row.version);
                // iOS may still list the version a Kiwi install that just completed replaces.
                let settling = other_version.is_some()
                    && install.origin == "kiwi"
                    && now - install.installed_at < INSTALL_GRACE;
                if !settling {
                    row.verified = true;
                    if let Some(version) = other_version {
                        row.version = version.clone();
                        row.build = app.build.clone();
                        row.release = None;
                    }
                }
            }
            None if install.removed_at.is_none()
                && (install.verified || now - install.installed_at > INSTALL_GRACE) =>
            {
                row.removed_at = Some(now);
                row.verified = false;
            }
            None => {}
        }
        if row != *install {
            changed.push(row);
        }
    }
    for app in apps {
        if installs.iter().any(|i| i.signed_bundle_id == app.bundle_id) {
            continue;
        }
        let Some(version) = &app.version else {
            continue;
        };
        let matched = catalog.iter().find(|c| {
            let id = &c.app().bundle_id;
            *id == app.bundle_id
                || team_id.is_some_and(|t| kiwi_signer::signed_bundle_id(id, t) == app.bundle_id)
        });
        if let Some(c) = matched {
            let source_app = c.app();
            changed.push(InstallRow {
                device_id: device_id.to_owned(),
                signed_bundle_id: app.bundle_id.clone(),
                source_id: c.source.id.clone(),
                bundle_id: source_app.bundle_id.clone(),
                name: source_app.name.clone(),
                icon_url: source_app.icon_url.as_ref().map(|u| u.to_string()),
                developer: source_app.developer.clone(),
                build_id: None,
                version: version.clone(),
                build: app.build.clone(),
                release: None,
                url_scheme: None,
                expires_at: None,
                origin: "device".into(),
                verified: true,
                installed_at: now,
                last_seen_at: Some(now),
                removed_at: None,
            });
        }
    }
    changed
}

/// Runs a devicectl sync: auto-link, then for each linked device record reachability, OS
/// version and installed apps. Only the sync's own device columns are written, and only
/// while the device is still linked to the same CoreDevice.
pub async fn sync(kiwi: &Arc<Kiwi>) -> ApiResult<()> {
    if !kiwi.devicectl_available.load(Ordering::Relaxed) {
        return Ok(());
    }
    let now = Utc::now();
    let cores = core_devices(kiwi, true).await;
    let devices = kiwi.db.devices()?;
    if let Some(core) = auto_link_candidate(&cores, &devices, kiwi.config.allow_simulators) {
        tracing::info!("linking {} to CoreDevice {}", devices[0].id, core.name);
        kiwi.db
            .set_device_link(&devices[0].id, Some(link_columns(core)))?;
    }
    let team = kiwi.team_id();
    let catalog = kiwi.catalog.apps(None, None);
    for device in kiwi.db.devices()? {
        let Some(core_id) = device.devicectl_id.clone() else {
            continue;
        };
        let record = |reachable: bool, name: Option<&str>, error: Option<&str>| {
            kiwi.db
                .record_device_sync(&device.id, &core_id, now, reachable, name, error)
        };
        let core = match cores.iter().find(|c| c.identifier == core_id) {
            None => {
                record(false, None, Some("devicectl no longer lists this device."))?;
                continue;
            }
            Some(core) if !core.reachable() => {
                let error = format!(
                    "Not reachable ({}).",
                    core.tunnel_state
                        .as_deref()
                        .or(core.pairing_state.as_deref())
                        .unwrap_or("unknown state")
                );
                record(false, Some(&core.name), Some(&error))?;
                continue;
            }
            Some(core) => core,
        };
        let error = match kiwi.devices.list_apps(&core_id).await {
            Ok(apps) => {
                let installs = kiwi.db.installs(&device.id, true)?;
                for row in reconcile(&device.id, &installs, &apps, &catalog, team.as_deref(), now) {
                    kiwi.db.save_install(&row)?;
                }
                None
            }
            Err(e) => Some(e),
        };
        if !record(true, Some(&core.name), error.as_deref())? {
            continue; // unlinked or relinked meanwhile
        }
        if let Some(v) = &core.os_version {
            kiwi.db.observe_device_os(&device.id, v, "devicectl")?;
        }
        if let Some(model) = &core.product_type {
            kiwi.db.set_device_model(&device.id, model)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{TEAM, install_of, release};
    use std::sync::atomic::{AtomicBool, AtomicUsize};

    #[derive(Default)]
    struct CountingDevices {
        whois_calls: AtomicUsize,
        core_calls: AtomicUsize,
        fail: AtomicBool,
        hold: AtomicBool,
        entered: tokio::sync::Notify,
    }

    impl DeviceTools for CountingDevices {
        fn whois(&self, _ip: IpAddr) -> BoxFuture<'_, Result<Option<TailscalePeer>, String>> {
            Box::pin(async {
                self.whois_calls.fetch_add(1, Ordering::SeqCst);
                self.entered.notify_one();
                if self.hold.load(Ordering::SeqCst) {
                    std::future::pending::<()>().await;
                }
                tokio::task::yield_now().await;
                if self.fail.load(Ordering::SeqCst) {
                    Err("offline".into())
                } else {
                    Ok(Some(TailscalePeer {
                        stable_id: "phone".into(),
                        name: "Phone".into(),
                        dns_name: "phone.example.ts.net".into(),
                        os: Some("iOS".into()),
                        user_login: None,
                        user_name: None,
                    }))
                }
            })
        }

        fn owner(&self) -> BoxFuture<'_, Result<Option<TailscaleUser>, String>> {
            Box::pin(async { Ok(None) })
        }

        fn devicectl_available(&self) -> BoxFuture<'_, bool> {
            Box::pin(async { true })
        }

        fn list_devices(&self) -> BoxFuture<'_, Result<Vec<CoreDevice>, String>> {
            Box::pin(async {
                self.core_calls.fetch_add(1, Ordering::SeqCst);
                tokio::task::yield_now().await;
                if self.fail.load(Ordering::SeqCst) {
                    Err("offline".into())
                } else {
                    Ok(vec![core("phone", true, true)])
                }
            })
        }

        fn list_apps<'a>(&'a self, _: &'a str) -> BoxFuture<'a, Result<Vec<DeviceApp>, String>> {
            Box::pin(async { Ok(Vec::new()) })
        }
    }

    #[tokio::test]
    async fn concurrent_whois_requests_share_successes_and_failures() {
        let caches = DeviceCaches::default();
        let tools = CountingDevices::default();
        let ip = "100.64.0.7".parse().unwrap();
        let (a, b, c) = tokio::join!(
            caches.whois(&tools, ip),
            caches.whois(&tools, ip),
            caches.whois(&tools, ip),
        );
        assert_eq!(a.unwrap(), b.unwrap());
        assert_eq!(c.unwrap().unwrap().stable_id, "phone");
        assert_eq!(tools.whois_calls.load(Ordering::SeqCst), 1);

        // An expired success is retried only twice for the entire group of requests.
        caches.whois.lock().unwrap().get_mut(&ip).unwrap().0 = Instant::now() - WHOIS_TTL;
        tools.fail.store(true, Ordering::SeqCst);
        let (a, b) = tokio::join!(caches.whois(&tools, ip), caches.whois(&tools, ip));
        assert!(a.is_err());
        assert!(b.is_err());
        assert_eq!(tools.whois_calls.load(Ordering::SeqCst), 3);

        *caches.whois_failed.lock().unwrap().get_mut(&ip).unwrap() =
            Instant::now() - WHOIS_ERROR_TTL;
        tools.fail.store(false, Ordering::SeqCst);
        assert!(caches.whois(&tools, ip).await.unwrap().is_some());
        assert_eq!(tools.whois_calls.load(Ordering::SeqCst), 4);
    }

    #[tokio::test]
    async fn cancelled_whois_does_not_block_other_addresses_or_future_requests() {
        let caches = Arc::new(DeviceCaches::default());
        let tools = Arc::new(CountingDevices::default());
        tools.hold.store(true, Ordering::SeqCst);
        let ip = "100.64.0.7".parse().unwrap();
        let lookup = tokio::spawn({
            let caches = caches.clone();
            let tools = tools.clone();
            async move { caches.whois(tools.as_ref(), ip).await }
        });
        tools.entered.notified().await;
        tools.hold.store(false, Ordering::SeqCst);
        // A hung lookup for one phone does not block another phone.
        let other = "100.64.0.8".parse().unwrap();
        assert!(
            tokio::time::timeout(Duration::from_secs(1), caches.whois(tools.as_ref(), other))
                .await
                .unwrap()
                .unwrap()
                .is_some()
        );
        lookup.abort();
        assert!(lookup.await.unwrap_err().is_cancelled());
        assert!(
            tokio::time::timeout(Duration::from_secs(1), caches.whois(tools.as_ref(), ip))
                .await
                .unwrap()
                .unwrap()
                .is_some()
        );
        assert_eq!(tools.whois_calls.load(Ordering::SeqCst), 3);
        assert_eq!(caches.whois_queries.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn concurrent_core_refreshes_share_results_and_preserve_forced_refresh() {
        let caches = DeviceCaches::default();
        let tools = CountingDevices::default();
        let (a, b) = tokio::join!(
            caches.core_devices(&tools, false),
            caches.core_devices(&tools, false),
        );
        assert_eq!(a[0].identifier, b[0].identifier);
        assert_eq!(tools.core_calls.load(Ordering::SeqCst), 1);
        let (a, b) = tokio::join!(
            caches.core_devices(&tools, true),
            caches.core_devices(&tools, true),
        );
        assert_eq!(a[0].identifier, b[0].identifier);
        assert_eq!(tools.core_calls.load(Ordering::SeqCst), 2);
        caches.core_devices(&tools, false).await;
        assert_eq!(tools.core_calls.load(Ordering::SeqCst), 2);
        caches.core_devices(&tools, true).await;
        assert_eq!(tools.core_calls.load(Ordering::SeqCst), 3);

        // Failed refreshes keep the last known device list, as before.
        tools.fail.store(true, Ordering::SeqCst);
        assert_eq!(
            caches.core_devices(&tools, true).await[0].identifier,
            "phone"
        );
        tools.fail.store(false, Ordering::SeqCst);
        caches.core.lock().unwrap().as_mut().unwrap().0 = Instant::now() - CORE_DEVICES_TTL;
        caches.core_devices(&tools, false).await;
        assert_eq!(tools.core_calls.load(Ordering::SeqCst), 5);
    }

    fn device() -> DeviceRow {
        DeviceRow::new("ts:n1".into(), "tailscale", Utc::now())
    }

    #[test]
    fn forwarded_client_only_from_loopback() {
        let lo: SocketAddr = "127.0.0.1:5000".parse().unwrap();
        let lo6: SocketAddr = "[::ffff:127.0.0.1]:5000".parse().unwrap();
        let lan: SocketAddr = "192.168.1.2:5000".parse().unwrap();
        let ts: IpAddr = "100.101.102.103".parse().unwrap();
        assert_eq!(
            forwarded_client(Some(lo), Some("100.101.102.103")),
            Some(ts)
        );
        assert_eq!(
            forwarded_client(Some(lo6), Some("1.2.3.4, 100.101.102.103")),
            Some(ts)
        );
        assert_eq!(forwarded_client(Some(lan), Some("100.101.102.103")), None);
        assert_eq!(forwarded_client(Some(lo), None), None);
        assert_eq!(forwarded_client(Some(lo), Some("garbage")), None);
        assert_eq!(forwarded_client(None, Some("100.101.102.103")), None);
    }

    #[test]
    fn parses_device_cookie() {
        let id = format!("local:{}", random_token());
        assert_eq!(
            cookie_device_id(Some(&format!("a=b; kiwi_device={id}; c=d"))),
            Some(id)
        );
        assert_eq!(cookie_device_id(Some("kiwi_device=ts:abc")), None);
        assert_eq!(cookie_device_id(Some("kiwi_device=local:../../etc")), None);
        assert_eq!(cookie_device_id(None), None);
    }

    #[test]
    fn os_version_never_downgrades_the_source() {
        let mut d = device();
        let safari = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_6 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/27.0 Mobile/15E148 Safari/604.1";
        assert!(observe_user_agent(&mut d, Some(safari)));
        assert_eq!(
            (d.os_version.as_deref(), d.os_version_source.as_deref()),
            (Some("27.0"), Some("safari"))
        );
        assert!(!observe_user_agent(&mut d, Some(safari)));
        assert!(observe_os(&mut d, "27.0.2", "manual"));
        assert!(!observe_user_agent(&mut d, Some(safari)));
        let installer =
            "itunesstored/1.0 iOS/27.0.1 model/iPhone19,4 hwp/t8150 build/24A434 (6; dt:332) AMS/1";
        assert!(observe_user_agent(&mut d, Some(installer)));
        assert_eq!(d.os_version.as_deref(), Some("27.0.1"));
        assert_eq!(d.os_version_source.as_deref(), Some("installer"));
        assert_eq!(d.model.as_deref(), Some("iPhone19,4"));
        assert!(!observe_os(&mut d, "26.0", "manual"));
        assert!(observe_os(&mut d, "27.1", "devicectl"));
        assert!(!observe_user_agent(&mut d, Some(installer)));
        assert_eq!(d.os_version.as_deref(), Some("27.1"));
        assert!(!observe_user_agent(&mut d, Some("curl/8.7.1")));
        let profile = device_profile(&d);
        assert_eq!((profile.os_version.as_str(), profile.family), ("27.1", 1));
        assert_eq!(profile.platform, "ios");
        // An Android browser never turns an iPhone into an Android device, nor gives it
        // an Android version.
        let chrome = "Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Mobile Safari/537.36";
        assert!(!observe_user_agent(&mut d, Some(chrome)));
        let firefox = "Mozilla/5.0 (Android 15; Mobile; rv:143.0) Gecko/143.0 Firefox/143.0";
        assert!(!observe_user_agent(&mut d, Some(firefox)));
        assert_eq!(d.os_version.as_deref(), Some("27.1"));
        assert_eq!(platform(&d), (Platform::Ios, PlatformSource::Detected));
    }

    #[test]
    fn android_browsers_set_the_platform() {
        let mut d = device();
        assert_eq!(platform(&d), (Platform::Ios, PlatformSource::Assumed));
        let unknown = device_profile(&d);
        assert_eq!(
            (unknown.platform.as_str(), unknown.os_version.as_str()),
            ("ios", "")
        );
        let chrome = "Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Mobile Safari/537.36";
        assert!(observe_user_agent(&mut d, Some(chrome)));
        assert_eq!(platform(&d), (Platform::Android, PlatformSource::Detected));
        assert_eq!(d.model.as_deref(), Some("Android"));
        assert!(d.os_version.is_none());
        assert!(!observe_user_agent(&mut d, Some(chrome)));
        let profile = device_profile(&d);
        assert_eq!((profile.platform.as_str(), profile.family), ("android", 1));
        let firefox = "Mozilla/5.0 (Android 15; Mobile; rv:143.0) Gecko/143.0 Firefox/143.0";
        assert!(observe_user_agent(&mut d, Some(firefox)));
        assert_eq!(
            (d.os_version.as_deref(), d.os_version_source.as_deref()),
            (Some("15"), Some("android"))
        );
        // Desktop mode sends a Linux User-Agent; the device stays an Android one.
        let desktop = "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";
        assert!(!observe_user_agent(&mut d, Some(desktop)));
        assert_eq!(effective_platform(&d), Platform::Android);
        // An iOS User-Agent doesn't turn it into an iPhone either.
        let safari = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_6 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/27.0 Mobile/15E148 Safari/604.1";
        assert!(!observe_user_agent(&mut d, Some(safari)));
        assert_eq!(d.os_version.as_deref(), Some("15"));
        assert_eq!(effective_platform(&d), Platform::Android);
    }

    #[test]
    fn kiwis_android_app_reports_the_exact_release() {
        let mut d = device();
        let app = "Kiwi-Android/1.2.0 (12; Android 15; SDK 35; Google Pixel 9a)";
        assert_eq!(Client::of(Some(app)), Client::AndroidApp);
        assert_eq!(Client::of(None), Client::Web);
        assert!(observe_user_agent(&mut d, Some(app)));
        assert_eq!(platform(&d), (Platform::Android, PlatformSource::Detected));
        assert_eq!(
            (
                d.model.as_deref(),
                d.os_version.as_deref(),
                d.os_version_source.as_deref()
            ),
            (Some("Android"), Some("15"), Some("android_app"))
        );
        // The app outranks a browser's guess, and the owner's own entry outranks the app.
        let firefox = "Mozilla/5.0 (Android 16; Mobile; rv:143.0) Gecko/143.0 Firefox/143.0";
        assert!(!observe_user_agent(&mut d, Some(firefox)));
        assert_eq!(d.os_version.as_deref(), Some("15"));
        assert!(observe_os(&mut d, "16", "manual"));
        assert!(!observe_user_agent(&mut d, Some(app)));
        assert_eq!(d.os_version.as_deref(), Some("16"));
    }

    #[test]
    fn the_owners_choice_wins_over_detection() {
        let mut d = device();
        // A desktop browser tells nothing: iOS is assumed until the owner chooses.
        let desktop = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/27.0 Safari/605.1.15";
        assert!(!observe_user_agent(&mut d, Some(desktop)));
        assert_eq!(platform(&d), (Platform::Ios, PlatformSource::Assumed));
        d.platform = Some(Platform::Android);
        assert_eq!(platform(&d), (Platform::Android, PlatformSource::Chosen));
        assert_eq!(device_profile(&d).platform, "android");
        // Also over what Tailscale or a User-Agent reported.
        let mut iphone = device();
        iphone.detected_platform = Some(Platform::Ios);
        iphone.platform = Some(Platform::Android);
        assert_eq!(
            platform(&iphone),
            (Platform::Android, PlatformSource::Chosen)
        );
        let json = device_json(&iphone, Client::Web);
        assert_eq!(json.detected_platform, Some(Platform::Ios));
        assert_eq!(
            serde_json::to_value(&json).unwrap()["platform_source"],
            "chosen"
        );
        iphone.platform = None;
        assert_eq!(platform(&iphone), (Platform::Ios, PlatformSource::Detected));
        // Tailscale names the OS of iPhones and iPads alike.
        assert_eq!(tailscale_platform("iOS"), Some(Platform::Ios));
        assert_eq!(tailscale_platform("android"), Some(Platform::Android));
        assert_eq!(tailscale_platform("macOS"), None);
        assert_eq!(tailscale_platform("linux"), None);
    }

    fn core(id: &str, physical: bool, paired: bool) -> CoreDevice {
        CoreDevice {
            identifier: id.into(),
            name: id.into(),
            udid: None,
            model: None,
            product_type: None,
            platform: Some("iOS".into()),
            os_version: Some("27.0".into()),
            physical,
            pairing_state: Some(if paired { "paired" } else { "unpaired" }.into()),
            tunnel_state: Some("connected".into()),
            transport: None,
            last_connection: None,
        }
    }

    #[test]
    fn auto_link_needs_exactly_one_of_each() {
        let one = vec![device()];
        let sims = vec![core("sim1", false, true), core("sim2", false, true)];
        assert!(auto_link_candidate(&sims, &one, false).is_none());
        assert!(auto_link_candidate(&sims, &one, true).is_none());
        let phone = vec![core("sim1", false, true), core("phone", true, true)];
        assert_eq!(
            auto_link_candidate(&phone, &one, false).unwrap().identifier,
            "phone"
        );
        assert!(auto_link_candidate(&[core("phone", true, false)], &one, false).is_none());
        let two = vec![
            device(),
            DeviceRow::new("local:x".into(), "browser", Utc::now()),
        ];
        assert!(auto_link_candidate(&phone, &two, false).is_none());
        let mut linked = device();
        linked.devicectl_id = Some("phone".into());
        assert!(auto_link_candidate(&phone, &[linked], false).is_none());
    }

    fn device_app(bundle_id: &str, version: &str) -> DeviceApp {
        DeviceApp {
            bundle_id: bundle_id.into(),
            name: None,
            version: Some(version.into()),
            build: None,
            removable: Some(true),
            path: None,
        }
    }

    #[test]
    fn reconcile_marks_verified_removed_and_outside_installs() {
        let now = Utc::now();
        let signed = format!("com.example.app.{TEAM}");
        let mut fresh = install_of(&release("1.0", None), "kiwi", now);
        fresh.signed_bundle_id = "fresh.app".into();
        let present = install_of(
            &release("1.0", None),
            "kiwi",
            now - chrono::Duration::hours(1),
        );
        let mut gone = present.clone();
        gone.signed_bundle_id = "gone.app".into();

        // Installed and seen: verified. Missing but just installed: kept. Missing: removed.
        let rows = reconcile(
            "ts:test",
            &[present.clone(), fresh.clone(), gone.clone()],
            &[device_app(&signed, "1.0")],
            &[],
            Some(TEAM),
            now,
        );
        assert_eq!(rows.len(), 2);
        assert!(rows[0].verified && rows[0].signed_bundle_id == signed);
        assert_eq!(rows[1].signed_bundle_id, "gone.app");
        assert_eq!(rows[1].removed_at, Some(now));
        // Unchanged rows are not rewritten; a reinstall clears removed_at.
        assert!(
            reconcile(
                "ts:test",
                &rows[..1],
                &[device_app(&signed, "1.0")],
                &[],
                None,
                now
            )
            .is_empty()
        );
        let back = reconcile(
            "ts:test",
            &rows[1..],
            &[device_app("gone.app", "2.0")],
            &[],
            None,
            now,
        );
        assert_eq!(back[0].removed_at, None);
        assert_eq!(back[0].version, "2.0");
        assert!(back[0].release.is_none());
    }

    #[test]
    fn reconcile_keeps_the_version_of_a_kiwi_install_that_just_completed() {
        let now = Utc::now();
        let signed = format!("com.example.app.{TEAM}");
        let fresh = install_of(
            &release("2.0", None),
            "kiwi",
            now - chrono::Duration::minutes(2),
        );
        // iOS may still be replacing 1.0 when devicectl lists the apps.
        let rows = reconcile(
            "ts:test",
            std::slice::from_ref(&fresh),
            &[device_app(&signed, "1.0")],
            &[],
            Some(TEAM),
            now,
        );
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].version, "2.0");
        assert!(rows[0].release.is_some());
        assert!(!rows[0].verified, "not seen with its version yet");
        // ...so it is not marked removed if the next listing misses it.
        assert!(reconcile("ts:test", &rows, &[], &[], Some(TEAM), now).is_empty());
        // Later, or for apps seen by devicectl, the device's version wins.
        for (origin, age) in [("kiwi", 30), ("device", 2)] {
            let mut install = fresh.clone();
            install.origin = origin.into();
            install.installed_at = now - chrono::Duration::minutes(age);
            let rows = reconcile(
                "ts:test",
                &[install],
                &[device_app(&signed, "1.0")],
                &[],
                Some(TEAM),
                now,
            );
            assert_eq!(rows[0].version, "1.0", "{origin}");
        }
    }

    #[test]
    fn reconcile_records_source_apps_installed_outside_kiwi() {
        let now = Utc::now();
        let source = std::sync::Arc::new(crate::testutil::source_row(
            "src",
            vec![crate::testutil::app(&[("1.0", None)])],
        ));
        let catalog = vec![CatalogApp { source, index: 0 }];
        let apps = [
            device_app("com.example.app", "0.9"),
            device_app("com.unrelated", "1.0"),
        ];
        let rows = reconcile("ts:test", &[], &apps, &catalog, Some(TEAM), now);
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(
            (
                row.signed_bundle_id.as_str(),
                row.origin.as_str(),
                row.version.as_str()
            ),
            ("com.example.app", "device", "0.9")
        );
        assert!(row.verified && row.url_scheme.is_none());
        // Kiwi's signed id without a Kiwi record (e.g. after "Forget").
        let signed = format!("com.example.app.{TEAM}");
        let rows = reconcile(
            "ts:test",
            &[],
            &[device_app(&signed, "1.0")],
            &catalog,
            Some(TEAM),
            now,
        );
        assert_eq!(rows[0].signed_bundle_id, signed);
        assert_eq!(rows[0].bundle_id, "com.example.app");
    }
}
