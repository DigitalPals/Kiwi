//! Kiwi's server: AltStore sources, re-signed builds, OTA installs, devices, web push and
//! the web app. See docs/architecture.md and docs/api.md.

pub mod actions;
pub mod android;
pub mod art;
pub mod automation;
pub mod builds;
pub mod catalog;
pub mod community;
pub mod config;
pub mod db;
pub mod devices;
pub mod error;
pub mod kiwi_app;
pub mod linked;
pub mod ota;
pub mod owner;
pub mod peers;
pub mod push;
pub mod routes;
pub mod scheduler;
pub mod settings;
pub mod signing;
pub mod sources;
pub mod store;
#[cfg(test)]
mod testutil;
pub mod views;

use std::{
    future::Future,
    io::{Read, Write},
    path::{Path, PathBuf},
    sync::{Arc, RwLock, atomic::AtomicBool},
    time::Duration,
};

use axum::{Router, extract::connect_info::IntoMakeServiceWithConnectInfo, http::HeaderMap};
use chrono::Utc;
use kiwi_device::Tailscale;
use kiwi_source::{CancellationToken, ClientOptions, SourceClient};
use tokio::sync::Notify;

pub use config::{Args, Config};
pub use routes::router;

use crate::{
    android::{AndroidAppCache, PackageReports},
    builds::Jobs,
    catalog::Catalog,
    config::{AllowedHosts, valid_forwarded_host},
    db::Db,
    devices::{DeviceCaches, DeviceTools, SystemDevices},
    ota::ServedRanges,
    peers::{Connection, PeerUids, ProcNet},
    push::{NoticeBackoff, PushTransport, WebPush},
    settings::Settings,
    signing::{SIGNING_SCAN_TIMEOUT, Signer, SigningInfo, SystemSigner, scan_signing},
    store::Store,
};

/// Files under the data directory.
#[derive(Debug, Clone)]
pub struct Paths {
    pub db: PathBuf,
    pub downloads: PathBuf,
    pub builds: PathBuf,
    pub work: PathBuf,
    pub icons: PathBuf,
    /// Source artwork the web app shows ([`art`]), and the key its URLs are signed with.
    pub art: PathBuf,
    pub art_key: PathBuf,
    pub vapid: PathBuf,
    /// Kiwi's Android app (`kiwi.apk`, `kiwi.json`), put there by whatever provides it.
    pub android_app: PathBuf,
    /// Uploaded signing identity and provisioning profiles.
    pub signing: kiwi_signer::SigningDir,
}

impl Paths {
    fn new(data: &Path) -> Self {
        Self {
            db: data.join("kiwi.db"),
            downloads: data.join("downloads"),
            builds: data.join("builds"),
            work: data.join("work"),
            icons: data.join("icons"),
            art: data.join("art"),
            art_key: data.join("art.key"),
            vapid: data.join("vapid.json"),
            android_app: data.join("android-app"),
            signing: signing_dir(data),
        }
    }
}

/// `<data>/signing`: the uploaded `.p12` identity and provisioning profiles.
pub fn signing_dir(data: &Path) -> kiwi_signer::SigningDir {
    kiwi_signer::SigningDir::new(data.join("signing"))
}

/// Everything the server needs; platform pieces are injectable for tests.
pub struct Options {
    pub config: Config,
    pub source_client: ClientOptions,
    pub signer: Arc<dyn Signer>,
    pub devices: Arc<dyn DeviceTools>,
    /// Who made a loopback connection (off macOS).
    pub peers: Arc<dyn PeerUids>,
    /// `None`: real Web Push with the VAPID key in the data directory.
    pub push: Option<Arc<dyn PushTransport>>,
    /// Base URL of the iTunes Search API ([`store::DEFAULT_URL`]).
    pub app_store_url: String,
}

impl Options {
    /// Production options from command-line arguments.
    pub fn from_args(args: &Args) -> Result<Self, String> {
        let config = args.config()?;
        let tailscale = Tailscale::new(&args.tailscale_socket, &args.tailscale_bin)
            .map_err(|e| e.to_string())?;
        if tailscale.uses_localapi() {
            tracing::info!("asking tailscaled through {}", tailscale.socket().display());
        } else {
            tracing::info!(
                "no tailscaled socket at {}: asking Tailscale with {}",
                tailscale.socket().display(),
                tailscale.bin().display()
            );
        }
        let signer = Arc::new(SystemSigner {
            dir: signing_dir(&config.data_dir),
        });
        Ok(Self {
            config,
            source_client: ClientOptions::default(),
            signer,
            devices: Arc::new(SystemDevices { tailscale }),
            peers: Arc::new(ProcNet),
            push: None,
            app_store_url: store::DEFAULT_URL.into(),
        })
    }
}

pub struct Kiwi {
    pub config: Config,
    /// The public URL: `--public-url`, or this machine's Tailscale name once Tailscale told
    /// it (see [`learn_public_url`]).
    public: RwLock<Option<String>>,
    /// Host names requests may be addressed to (of the public URL and the listen address).
    hosts: RwLock<AllowedHosts>,
    pub paths: Paths,
    pub db: Db,
    pub catalog: Catalog,
    pub sources: SourceClient,
    pub store: Store,
    pub signer: Arc<dyn Signer>,
    signing: RwLock<SigningInfo>,
    /// Held while a signing scan runs, including one that timed out.
    signing_scan: Arc<tokio::sync::Mutex<()>>,
    team: RwLock<Option<String>>,
    pub devices: Arc<dyn DeviceTools>,
    pub peers: Arc<dyn PeerUids>,
    pub caches: DeviceCaches,
    pub devicectl_available: AtomicBool,
    pub push: Arc<dyn PushTransport>,
    /// Retry schedule of notifications whose delivery failed.
    pub notices: NoticeBackoff,
    pub vapid_public_key: String,
    settings: RwLock<Settings>,
    pub jobs: Jobs,
    pub automation: automation::Automation,
    /// The Morphe community list and whether community builds can run here.
    pub community: community::Community,
    pub owner: owner::Owner,
    /// IPA byte ranges served per install token (completion of ranged downloads).
    pub served: ServedRanges,
    /// Install tokens replaced recently, which still lead to their build.
    pub retired: ota::RetiredTokens,
    /// Kiwi's Android app, checked when its files change.
    pub android_app: AndroidAppCache,
    /// The last package report of each Android device.
    pub package_reports: PackageReports,
    /// One refresh per source at a time ([`sources::refresh`]).
    pub refreshes: sources::RefreshLocks,
    /// Cancelled on shutdown.
    pub cancel: CancellationToken,
    /// Wakes the scheduler (a build finished, a source changed, ...).
    pub kick: Notify,
}

/// Takes `<data>/kiwi.lock` for as long as the returned file is open, so a second Kiwi on
/// the same data directory stops before its startup cleanup (clearing `work/`, failing
/// running builds) damages the running one.
pub fn lock_data_dir(data: &Path) -> Result<std::fs::File, String> {
    let path = data.join("kiwi.lock");
    let error = |e: std::io::Error| format!("{}: {e}", path.display());
    std::fs::create_dir_all(data).map_err(error)?;
    let mut file = std::fs::File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(error)?;
    match file.try_lock() {
        Ok(()) => {}
        Err(std::fs::TryLockError::WouldBlock) => {
            let mut pid = String::new();
            let _ = file.read_to_string(&mut pid);
            return Err(format!(
                "Another Kiwi (pid {}) is already running with data directory {}",
                Some(pid.trim())
                    .filter(|p| !p.is_empty())
                    .unwrap_or("unknown"),
                data.display()
            ));
        }
        Err(std::fs::TryLockError::Error(e)) => return Err(error(e)),
    }
    // For the message above; the lock itself is what counts.
    let _ = file
        .set_len(0)
        .and_then(|()| writeln!(file, "{}", std::process::id()));
    Ok(file)
}

/// The Web Push key pair. An unreadable `vapid.json` is moved aside and replaced rather than
/// stopping the server; subscriptions made with the old key cannot be used any more, so
/// they are deleted (each device enables notifications again).
fn load_vapid(path: &Path, db: &Db) -> Result<kiwi_push::Vapid, String> {
    let error = match kiwi_push::Vapid::load_or_create(path) {
        Err(kiwi_push::Error::KeyFile(e)) => e,
        other => return other.map_err(|e| format!("{}: {e}", path.display())),
    };
    let aside = path.with_file_name(format!(
        "vapid.json.corrupt-{}",
        Utc::now().format("%Y%m%dT%H%M%SZ")
    ));
    std::fs::rename(path, &aside).map_err(|e| {
        format!(
            "{} is invalid ({error}) and could not be moved aside: {e}",
            path.display()
        )
    })?;
    let vapid = kiwi_push::Vapid::load_or_create(path).map_err(|e| e.to_string())?;
    let deleted = db.delete_all_subscriptions().map_err(|e| e.to_string())?;
    tracing::error!(
        "{} was invalid ({error}); moved it to {} and created a new Web Push key. \
         Deleted {deleted} push subscription(s) made with the old key: turn notifications \
         on again on each device.",
        path.display(),
        aside.display()
    );
    Ok(vapid)
}

fn prepare_dirs(paths: &Paths, data: &Path) -> std::io::Result<()> {
    for dir in [
        data,
        &paths.downloads,
        &paths.builds,
        &paths.icons,
        &paths.art,
    ] {
        std::fs::create_dir_all(dir)?;
    }
    // Temporary files of interrupted signing and downloads.
    let _ = std::fs::remove_dir_all(&paths.work);
    std::fs::create_dir_all(&paths.work)?;
    for entry in std::fs::read_dir(&paths.downloads)?.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.starts_with("source-") {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    Ok(())
}

impl Kiwi {
    pub async fn new(opts: Options) -> Result<Arc<Self>, String> {
        let config = opts.config;
        let paths = Paths::new(&config.data_dir);
        prepare_dirs(&paths, &config.data_dir)
            .map_err(|e| format!("{}: {e}", config.data_dir.display()))?;
        let db = Db::open(&paths.db)?;
        let interrupted = db
            .fail_interrupted_builds(Utc::now())
            .map_err(|e| e.to_string())?;
        if interrupted > 0 {
            tracing::warn!("{interrupted} build(s) were interrupted by a restart");
        }
        for b in db.builds().map_err(|e| e.to_string())? {
            if b.ipa_path.is_none() {
                let _ = std::fs::remove_dir_all(paths.builds.join(&b.id));
            }
        }
        let catalog = Catalog::default();
        let rows = db.sources().map_err(|e| e.to_string())?;
        let now = Utc::now();
        for row in &rows {
            let apps = row.source.catalog.apps.iter();
            db.record_apps(&row.id, apps.map(|a| a.bundle_id.as_str()), now)
                .map_err(|e| e.to_string())?;
        }
        catalog.load(rows);
        let stored = db.settings().map_err(|e| e.to_string())?;
        let builds_chosen = stored.iter().any(|(key, _)| key == "kiwi_builds");
        let mut defaults = Settings::default();
        if let Some(country) = &config.country {
            defaults.app_store_country = country.clone();
        }
        let mut settings = Settings::from_pairs(defaults, stored);
        if let Err(e) = art::load_key(&paths.art_key) {
            tracing::warn!("artwork URLs change at every start: {e}");
        }
        let vapid = load_vapid(&paths.vapid, &db)?;
        let vapid_public_key = vapid.public_key();
        let push = match opts.push {
            Some(push) => push,
            None => {
                let subject = config
                    .public_url
                    .clone()
                    .filter(|u| u.starts_with("https://"))
                    .unwrap_or_else(|| "mailto:kiwi@localhost".into());
                Arc::new(WebPush::new(
                    kiwi_push::PushSender::new(vapid, subject).map_err(|e| e.to_string())?,
                ))
            }
        };
        let sources = SourceClient::with_options(opts.source_client).map_err(|e| e.to_string())?;
        let store = Store::new(&opts.app_store_url)?;
        let automation = automation::Automation::load(&db, &config.data_dir)?;
        // Kiwi Builds became a setting, off by default; a Kiwi that builds apps keeps it on.
        if !builds_chosen && automation.snapshot().apps.values().any(|a| a.added) {
            settings.kiwi_builds = true;
            db.save_settings(&settings.pairs())
                .map_err(|e| e.to_string())?;
        }
        let devicectl = opts.devices.devicectl_available().await;
        let team = db.meta("team_id").map_err(|e| e.to_string())?;
        let owner = owner::Owner::new(
            config.owner.clone(),
            db.meta("owner_login")
                .map_err(|e| e.to_string())?
                .filter(|o| !o.is_empty()),
        );
        let hosts = AllowedHosts::new(&fallback_url(&config), config.listen);
        let kiwi = Arc::new(Self {
            public: RwLock::new(config.public_url.clone()),
            hosts: RwLock::new(hosts),
            paths,
            db,
            catalog,
            sources,
            store,
            signer: opts.signer,
            signing: RwLock::default(),
            signing_scan: Arc::default(),
            team: RwLock::new(team),
            devices: opts.devices,
            peers: opts.peers,
            caches: DeviceCaches::default(),
            devicectl_available: AtomicBool::new(devicectl),
            push,
            notices: NoticeBackoff::default(),
            vapid_public_key,
            settings: RwLock::new(settings),
            jobs: Jobs::default(),
            automation,
            community: community::Community::load(&config.data_dir),
            owner,
            served: ServedRanges::default(),
            retired: ota::RetiredTokens::default(),
            android_app: AndroidAppCache::default(),
            package_reports: PackageReports::default(),
            refreshes: sources::RefreshLocks::default(),
            cancel: CancellationToken::new(),
            kick: Notify::new(),
            config,
        });
        if let Err(e) = automation::retire(&kiwi) {
            tracing::warn!("could not update the Kiwi Builds source: {e}");
        }
        owner::refresh(&kiwi, true).await;
        let info = kiwi.rescan_signing().await;
        match info.error(Utc::now()) {
            Some(e) => tracing::warn!("signing unavailable: {e}"),
            None => tracing::info!(
                "signing with {} identities and {} profiles",
                info.identities.len(),
                info.profiles.len()
            ),
        }
        Ok(kiwi)
    }

    pub fn settings(&self) -> Settings {
        self.settings.read().unwrap().clone()
    }

    pub fn set_settings(&self, settings: Settings) {
        *self.settings.write().unwrap() = settings;
    }

    pub fn signing_info(&self) -> SigningInfo {
        self.signing.read().unwrap().clone()
    }

    /// Re-reads identities and profiles (blocking work on a worker thread), giving up after
    /// [`SIGNING_SCAN_TIMEOUT`] so a blocked keychain cannot stall startup, the scheduler or
    /// a build.
    pub async fn rescan_signing(&self) -> SigningInfo {
        let mut info = scan_signing(
            self.signer.clone(),
            self.signing_scan.clone(),
            &self.signing_info(),
            SIGNING_SCAN_TIMEOUT,
        )
        .await;
        info.uploads = Some(self.paths.signing.clone());
        if let Some(team) = info.team_id(Utc::now()) {
            let mut current = self.team.write().unwrap();
            if current.as_ref() != Some(&team) {
                if let Err(e) = self.db.set_meta("team_id", &team) {
                    tracing::warn!("could not save the team id: {e}");
                }
                *current = Some(team);
            }
        }
        *self.signing.write().unwrap() = info.clone();
        info
    }

    /// Team of the signing identity (remembered when the keychain is unavailable).
    pub fn team_id(&self) -> Option<String> {
        self.team.read().unwrap().clone()
    }

    /// `--public-url` (or this machine's Tailscale name), else `https://<X-Forwarded-Host>`,
    /// else the listen address.
    pub fn public_url(&self, headers: &HeaderMap) -> String {
        if let Some(url) = self.known_public_url() {
            return url;
        }
        headers
            .get("x-forwarded-host")
            .and_then(|v| v.to_str().ok())
            .filter(|h| valid_forwarded_host(h))
            .map_or_else(
                || format!("http://{}", self.config.listen),
                |h| format!("https://{h}"),
            )
    }

    pub fn default_public_url(&self) -> String {
        self.public_url(&HeaderMap::new())
    }

    /// `--public-url`, or this machine's Tailscale name once known.
    pub fn known_public_url(&self) -> Option<String> {
        self.public.read().unwrap().clone()
    }

    /// Whether a request may name `host` (`Host`) and `forwarded` (`X-Forwarded-Host`), see
    /// [`AllowedHosts`].
    pub fn allows_host(&self, host: Option<&str>, forwarded: Option<&str>) -> bool {
        self.hosts.read().unwrap().allows(host, forwarded)
    }

    /// Takes the public URL Tailscale named after Kiwi started: requests may name its host
    /// from now on, and links and Web Push use it.
    pub fn set_public_url(&self, url: String) {
        *self.hosts.write().unwrap() = AllowedHosts::new(&url, self.config.listen);
        if url.starts_with("https://") {
            self.push.set_subject(&url);
        }
        *self.public.write().unwrap() = Some(url);
    }

    /// Cancels builds and the scheduler and waits for them (bounded).
    pub async fn shutdown(&self) {
        self.cancel.cancel();
        self.jobs.tracker.close();
        if tokio::time::timeout(Duration::from_secs(15), self.jobs.tracker.wait())
            .await
            .is_err()
        {
            tracing::warn!("background jobs did not stop in time");
        }
    }
}

async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("SIGTERM handler");
        tokio::select! {
            _ = ctrl_c => {}
            _ = term.recv() => {}
        }
    }
    #[cfg(not(unix))]
    let _ = ctrl_c.await;
}

/// `--public-url`, else the listen address (reachable only from this machine itself).
fn fallback_url(config: &Config) -> String {
    config
        .public_url
        .clone()
        .unwrap_or_else(|| format!("http://{}", config.listen))
}

/// `https://<this machine's MagicDNS name>`, the address `tailscale serve` answers on;
/// `Ok(None)` while Tailscale has no name for it.
async fn tailscale_url(tailscale: &Tailscale) -> Result<Option<String>, String> {
    match tailscale.dns_name().await {
        Ok(name) => Ok(name.map(|name| format!("https://{name}"))),
        Err(e) => Err(e.to_string()),
    }
}

/// The longest pause between two questions of [`learn_public_url`].
const LEARN_MAX_DELAY: Duration = Duration::from_secs(30);

/// Asks `ask` for the public URL until it has one, then takes it
/// ([`Kiwi::set_public_url`]). Kiwi needs it when it was not given `--public-url` and
/// Tailscale could not name this machine at start: tailscaled was still starting (at boot,
/// or a Tailscale container started after Kiwi's, before its socket existed) or not logged
/// in. Until then Kiwi answers only on its listen address. The pause between questions
/// starts at `first` and doubles up to 30 s. Stops on shutdown.
pub async fn learn_public_url<F, Fut>(kiwi: Arc<Kiwi>, mut ask: F, first: Duration)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<Option<String>, String>>,
{
    let mut delay = first;
    loop {
        tokio::select! {
            _ = kiwi.cancel.cancelled() => return,
            _ = tokio::time::sleep(delay) => {}
        }
        match ask().await {
            Ok(Some(url)) => {
                tracing::info!("public URL from Tailscale: {url}");
                kiwi.set_public_url(url);
                return;
            }
            Ok(None) => tracing::debug!("Tailscale has no name for this machine yet"),
            Err(e) => tracing::debug!("could not ask Tailscale for this machine's name: {e}"),
        }
        delay = (delay * 2).min(LEARN_MAX_DELAY);
    }
}

/// The service `axum::serve` runs: the router, with each connection's [`Connection`].
pub fn app(kiwi: Arc<Kiwi>) -> IntoMakeServiceWithConnectInfo<Router, Connection> {
    router(kiwi).into_make_service_with_connect_info::<Connection>()
}

/// Runs the server until SIGTERM/SIGINT.
pub async fn run(args: Args) -> Result<(), String> {
    if !args.listen.ip().is_loopback() {
        tracing::warn!(
            "listening on {}: Kiwi should listen on loopback only (tailscale serve exposes it); \
             requests from other computers are never the owner's",
            args.listen
        );
    }
    let mut options = Options::from_args(&args)?;
    if args.trust_local && !cfg!(target_os = "macos") {
        tracing::warn!(
            "--trust-local: every user and program on this computer counts as Kiwi's owner"
        );
    }
    // Without --public-url, Tailscale names the address; until it does, Kiwi keeps asking.
    let mut learn = None;
    if options.config.public_url.is_none() {
        let tailscale = Tailscale::new(&args.tailscale_socket, &args.tailscale_bin)
            .map_err(|e| e.to_string())?;
        match tailscale_url(&tailscale).await {
            Ok(Some(url)) => {
                tracing::info!("public URL from Tailscale: {url}");
                options.config.public_url = Some(url);
            }
            result => {
                let why = match result {
                    Err(e) => format!("could not ask Tailscale for this machine's name: {e}"),
                    _ => "Tailscale has no name for this machine yet".into(),
                };
                tracing::warn!(
                    "no public URL ({why}): Kiwi answers only on http://{} until Tailscale \
                     names this machine, and keeps asking (or pass --public-url)",
                    args.listen
                );
                learn = Some(tailscale);
            }
        }
    }
    // Held until the process exits.
    let _lock = lock_data_dir(&options.config.data_dir)?;
    let kiwi = Kiwi::new(options).await?;
    let listener = tokio::net::TcpListener::bind(args.listen)
        .await
        .map_err(|e| format!("Cannot listen on {}: {e}", args.listen))?;
    tracing::info!(
        "Kiwi {} listening on http://{} (data {}, web {})",
        env!("CARGO_PKG_VERSION"),
        args.listen,
        kiwi.config.data_dir.display(),
        kiwi.config.web_dir.display()
    );
    kiwi.jobs.tracker.spawn(scheduler::run(kiwi.clone()));
    if let Some(tailscale) = learn {
        let tailscale = Arc::new(tailscale);
        kiwi.jobs.tracker.spawn(learn_public_url(
            kiwi.clone(),
            move || {
                let tailscale = tailscale.clone();
                async move { tailscale_url(&tailscale).await }
            },
            Duration::from_secs(2),
        ));
    }
    tokio::spawn({
        let cancel = kiwi.cancel.clone();
        async move {
            shutdown_signal().await;
            tracing::info!("shutting down");
            cancel.cancel();
        }
    });
    let server = axum::serve(listener, app(kiwi.clone()))
        .with_graceful_shutdown(kiwi.cancel.clone().cancelled_owned());
    let deadline = async {
        kiwi.cancel.cancelled().await;
        tokio::time::sleep(Duration::from_secs(10)).await;
    };
    tokio::select! {
        result = server => result.map_err(|e| e.to_string())?,
        _ = deadline => tracing::warn!("closing connections that are still open"),
    }
    kiwi.shutdown().await;
    tracing::info!("stopped");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_second_instance_cannot_take_the_data_directory() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let first = lock_data_dir(&data).unwrap();
        let err = lock_data_dir(&data).unwrap_err();
        assert_eq!(
            err,
            format!(
                "Another Kiwi (pid {}) is already running with data directory {}",
                std::process::id(),
                data.display()
            )
        );
        drop(first);
        lock_data_dir(&data).unwrap();
    }

    #[test]
    fn an_invalid_vapid_key_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vapid.json");
        let db = Db::open_in_memory().unwrap();
        let key = load_vapid(&path, &db).unwrap().public_key();
        assert_eq!(load_vapid(&path, &db).unwrap().public_key(), key);

        db.save_subscription(&db::SubscriptionRow {
            endpoint: "https://web.push.apple.com/x".into(),
            device_id: "ts:a".into(),
            p256dh: "p".into(),
            auth: "a".into(),
            created_at: Utc::now(),
            last_success_at: None,
            failures: 0,
        })
        .unwrap();
        std::fs::write(&path, b"{not json").unwrap();
        let replaced = load_vapid(&path, &db).unwrap().public_key();
        assert_ne!(replaced, key);
        assert!(db.subscriptions("ts:a").unwrap().is_empty());
        let aside: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .filter(|n| n.starts_with("vapid.json.corrupt-"))
            .collect();
        assert_eq!(aside.len(), 1);
        assert_eq!(
            std::fs::read(dir.path().join(&aside[0])).unwrap(),
            b"{not json"
        );
    }
}
