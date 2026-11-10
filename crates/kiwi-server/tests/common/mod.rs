//! Test harness: a fixture source server, fake signer/devices/push, and a Kiwi server on
//! an ephemeral loopback port.
#![allow(dead_code)]

use std::{
    collections::{HashMap, VecDeque},
    io::{Cursor, Write},
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, AtomicUsize, Ordering},
    },
    time::Duration,
};

use axum::{Router, extract::State, http::header, response::IntoResponse, routing::get};
use chrono::{DateTime, Utc};
use kiwi_device::{CoreDevice, DeviceApp, TailscalePeer, TailscaleUser};
use kiwi_push::{Outcome, Subscription};
use kiwi_server::{
    Kiwi, Options,
    config::{Config, LocalTrust},
    devices::{BoxFuture, DeviceTools},
    peers::PeerUids,
    push::{PushTransport, SendError},
    signing::Signer,
};
use kiwi_signer::{Backend, Identity, Profile, SignReport, SignRequest, SignStep};
use kiwi_source::{CancellationToken, ClientOptions};
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

pub const TEAM: &str = "A1B2C3D4E5";
pub const BUNDLE: &str = "com.example.kiwi";
pub const PHONE_IP: &str = "100.64.0.7";
pub const PHONE_ID: &str = "ts:nPHONE1CNTRL";
pub const INSTALLER_UA: &str =
    "itunesstored/1.0 iOS/27.0.1 model/iPhone19,4 hwp/t8150 build/24A434 (6; dt:332) AMS/1";
pub const PNG: &[u8] = b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR fake icon";

/// Valid-looking subscription keys (65-byte P-256 point, 16-byte secret).
pub fn push_keys() -> Value {
    use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
    json!({ "p256dh": B64.encode([4u8; 65]), "auth": B64.encode([1u8; 16]) })
}

pub fn signed_id() -> String {
    format!("{BUNDLE}.{TEAM}")
}

/// A minimal IPA whose Info.plist matches the fixture feed.
pub fn ipa(version: &str, build: &str) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options =
        zip::write::SimpleFileOptions::default().last_modified_time(zip::DateTime::default());
    zip.start_file("Payload/Kiwi.app/Info.plist", options)
        .unwrap();
    write!(
        zip,
        r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>{BUNDLE}</string>
<key>CFBundleShortVersionString</key><string>{version}</string>
<key>CFBundleVersion</key><string>{build}</string>
<key>MinimumOSVersion</key><string>16.0</string>
<key>CFBundleSupportedPlatforms</key><array><string>iPhoneOS</string></array>
<key>UIDeviceFamily</key><array><integer>1</integer></array>
</dict></plist>"#
    )
    .unwrap();
    zip.start_file("Payload/Kiwi.app/Kiwi", options).unwrap();
    zip.write_all(&vec![0x42; 300_000]).unwrap();
    zip.finish().unwrap().into_inner()
}

/// A 2 × 2 PNG header: enough for Kiwi to take it as an app icon.
pub fn icon_png() -> Vec<u8> {
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    for (kind, data) in [
        (&b"IHDR"[..], &[0, 0, 0, 2, 0, 0, 0, 2, 8, 6, 0, 0, 0][..]),
        (b"IEND", &[]),
    ] {
        png.extend_from_slice(&(data.len() as u32).to_be_bytes());
        png.extend_from_slice(kind);
        png.extend_from_slice(data);
        png.extend_from_slice(&[0; 4]);
    }
    png
}

/// An IPA with a Home Screen name and an icon, as an app added by link would have.
pub fn linked_ipa(version: &str, build: &str) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options =
        zip::write::SimpleFileOptions::default().last_modified_time(zip::DateTime::default());
    zip.start_file("Payload/Linked.app/Info.plist", options)
        .unwrap();
    write!(
        zip,
        r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>{BUNDLE}</string>
<key>CFBundleDisplayName</key><string>Linked App</string>
<key>CFBundleShortVersionString</key><string>{version}</string>
<key>CFBundleVersion</key><string>{build}</string>
<key>MinimumOSVersion</key><string>16.0</string>
<key>CFBundleSupportedPlatforms</key><array><string>iPhoneOS</string></array>
<key>UIDeviceFamily</key><array><integer>1</integer></array>
<key>CFBundleIcons</key><dict><key>CFBundlePrimaryIcon</key><dict>
<key>CFBundleIconFiles</key><array><string>AppIcon60x60</string></array></dict></dict>
</dict></plist>"#
    )
    .unwrap();
    zip.start_file("Payload/Linked.app/AppIcon60x60@2x.png", options)
        .unwrap();
    zip.write_all(&icon_png()).unwrap();
    zip.start_file("Payload/Linked.app/Linked", options)
        .unwrap();
    zip.write_all(&vec![0x42; 300_000 + build.len()]).unwrap();
    zip.finish().unwrap().into_inner()
}

/// Serves `/page` (HTML), `/feed.json`, `/other.json` (another source with an app of the same bundle id),
/// `/ipa/app-<version>.ipa`, `/icon.png`, `/html` and `/broken.json`.
pub struct Fixture {
    pub base: String,
    /// The HTML `/page` serves.
    pub page: Arc<Mutex<String>>,
    pub feed: Arc<Mutex<Value>>,
    pub other: Arc<Mutex<Value>>,
    ipas: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    /// IPA downloads answer 503 while set.
    pub fail_ipas: Arc<AtomicBool>,
    /// `/feed.json` waits while set; `feed_requests` counts its requests.
    pub hold_feed: Arc<AtomicBool>,
    pub feed_requests: Arc<AtomicUsize>,
    /// App Store apps (iTunes Search API results) by storefront, for `/lookup` and `/search`.
    pub store: Arc<Mutex<HashMap<String, Vec<Value>>>>,
    /// Query strings of App Store requests; they answer 503 while `fail_store` is set.
    pub store_requests: Arc<Mutex<Vec<String>>>,
    pub fail_store: Arc<AtomicBool>,
    cancel: CancellationToken,
}

/// An iTunes Search API result for an iOS app.
pub fn store_item(bundle_id: &str, name: &str, version: &str) -> Value {
    json!({
        "kind": "software", "bundleId": bundle_id, "trackId": 1000 + bundle_id.len(),
        "trackName": name, "artistName": "Official Dev", "version": version,
        "currentVersionReleaseDate": "2026-10-20T08:00:00Z",
        "artworkUrl512": "https://is1-ssl.mzstatic.com/icon.png",
        "trackViewUrl": format!("https://apps.apple.com/app/id{}?uo=4", 1000 + bundle_id.len()),
    })
}

/// `/lookup?bundleId=a,b&country=xx` and `/search?term=..&country=xx`.
fn store_answer(s: &FixtureState, query: &str, search: bool) -> axum::response::Response {
    s.store_requests.lock().unwrap().push(query.to_owned());
    if s.fail_store.load(Ordering::SeqCst) {
        return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response();
    }
    let params: HashMap<String, String> = url::form_urlencoded::parse(query.as_bytes())
        .into_owned()
        .collect();
    let country = params.get("country").cloned().unwrap_or_default();
    let apps = s
        .store
        .lock()
        .unwrap()
        .get(&country)
        .cloned()
        .unwrap_or_default();
    let field = |app: &Value, key: &str| app[key].as_str().unwrap_or("").to_lowercase();
    let results: Vec<Value> = if search {
        let term = params
            .get("term")
            .cloned()
            .unwrap_or_default()
            .to_lowercase();
        apps.into_iter()
            .filter(|a| field(a, "trackName").contains(&term))
            .collect()
    } else {
        let ids: Vec<String> = params
            .get("bundleId")
            .map(|ids| ids.split(',').map(str::to_lowercase).collect())
            .unwrap_or_default();
        apps.into_iter()
            .filter(|a| ids.contains(&field(a, "bundleId")))
            .collect()
    };
    axum::Json(json!({ "resultCount": results.len(), "results": results })).into_response()
}

#[derive(Clone)]
struct FixtureState {
    page: Arc<Mutex<String>>,
    feed: Arc<Mutex<Value>>,
    other: Arc<Mutex<Value>>,
    ipas: Arc<Mutex<HashMap<String, Vec<u8>>>>,
    fail_ipas: Arc<AtomicBool>,
    hold_feed: Arc<AtomicBool>,
    feed_requests: Arc<AtomicUsize>,
    store: Arc<Mutex<HashMap<String, Vec<Value>>>>,
    store_requests: Arc<Mutex<Vec<String>>>,
    fail_store: Arc<AtomicBool>,
}

impl Fixture {
    pub async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let state = FixtureState {
            page: Arc::default(),
            feed: Arc::new(Mutex::new(Value::Null)),
            other: Arc::new(Mutex::new(Value::Null)),
            ipas: Arc::default(),
            fail_ipas: Arc::default(),
            hold_feed: Arc::default(),
            feed_requests: Arc::default(),
            store: Arc::default(),
            store_requests: Arc::default(),
            fail_store: Arc::default(),
        };
        let app = Router::new()
            .route(
                "/feed.json",
                get(|State(s): State<FixtureState>| async move {
                    s.feed_requests.fetch_add(1, Ordering::SeqCst);
                    while s.hold_feed.load(Ordering::SeqCst) {
                        tokio::time::sleep(Duration::from_millis(10)).await;
                    }
                    axum::Json(s.feed.lock().unwrap().clone())
                }),
            )
            .route(
                "/other.json",
                get(|State(s): State<FixtureState>| async move {
                    axum::Json(s.other.lock().unwrap().clone())
                }),
            )
            .route(
                "/ipa/{name}",
                get(
                    |State(s): State<FixtureState>,
                     axum::extract::Path(name): axum::extract::Path<String>| async move {
                        if s.fail_ipas.load(Ordering::SeqCst) {
                            return axum::http::StatusCode::SERVICE_UNAVAILABLE.into_response();
                        }
                        match s.ipas.lock().unwrap().get(&name) {
                            Some(bytes) => bytes.clone().into_response(),
                            None => axum::http::StatusCode::NOT_FOUND.into_response(),
                        }
                    },
                ),
            )
            .route(
                "/icon.png",
                get(|| async { ([(header::CONTENT_TYPE, "image/png")], PNG) }),
            )
            .route(
                "/page",
                get(|State(s): State<FixtureState>| async move {
                    axum::response::Html(s.page.lock().unwrap().clone())
                }),
            )
            .route(
                "/html",
                get(|| async { axum::response::Html("<html>login</html>") }),
            )
            .route("/broken.json", get(|| async { "{\"name\": 1, \"apps\": " }))
            .route(
                "/lookup",
                get(
                    |State(s): State<FixtureState>,
                     axum::extract::RawQuery(q): axum::extract::RawQuery| async move {
                        store_answer(&s, &q.unwrap_or_default(), false)
                    },
                ),
            )
            .route(
                "/search",
                get(
                    |State(s): State<FixtureState>,
                     axum::extract::RawQuery(q): axum::extract::RawQuery| async move {
                        store_answer(&s, &q.unwrap_or_default(), true)
                    },
                ),
            )
            .with_state(state.clone());
        let cancel = CancellationToken::new();
        let stop = cancel.clone();
        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(stop.cancelled_owned())
                .await
                .unwrap();
        });
        let fixture = Self {
            base,
            page: state.page,
            feed: state.feed,
            other: state.other,
            ipas: state.ipas,
            fail_ipas: state.fail_ipas,
            hold_feed: state.hold_feed,
            feed_requests: state.feed_requests,
            store: state.store,
            store_requests: state.store_requests,
            fail_store: state.fail_store,
            cancel,
        };
        fixture.set_releases(&[("1.0", "1")]);
        fixture.set_other_releases(&[("1.0", "1")]);
        fixture
    }

    /// Serves `bytes` at `/ipa/<name>`; returns its URL.
    pub fn put_ipa(&self, name: &str, bytes: Vec<u8>) -> String {
        self.ipas.lock().unwrap().insert(name.into(), bytes);
        format!("{}/ipa/{name}", self.base)
    }

    pub fn feed_url(&self) -> String {
        format!("{}/feed.json", self.base)
    }

    pub fn other_url(&self) -> String {
        format!("{}/other.json", self.base)
    }

    /// Replaces the app's releases (newest first) and serves their IPAs.
    pub fn set_releases(&self, releases: &[(&str, &str)]) {
        *self.feed.lock().unwrap() = self.feed_json("Test Source", "Kiwi Test", releases);
    }

    /// Releases of "Kiwi Other" in "Other Source": a different app with the same bundle id.
    pub fn set_other_releases(&self, releases: &[(&str, &str)]) {
        *self.other.lock().unwrap() = self.feed_json("Other Source", "Kiwi Other", releases);
    }

    fn feed_json(&self, source: &str, name: &str, releases: &[(&str, &str)]) -> Value {
        let versions: Vec<Value> = releases
            .iter()
            .map(|(version, build)| {
                let bytes = ipa(version, build);
                let name = format!("app-{version}.ipa");
                let size = bytes.len();
                self.ipas.lock().unwrap().insert(name.clone(), bytes);
                json!({
                    "version": version, "buildVersion": build, "date": "2026-10-01",
                    "localizedDescription": format!("Release {version}"),
                    "downloadURL": format!("{}/ipa/{name}", self.base),
                    "size": size, "minOSVersion": "16.0"
                })
            })
            .collect();
        json!({
            "name": source,
            "identifier": source.to_lowercase().replace(' ', "."),
            "subtitle": "Fixtures",
            "iconURL": format!("{}/icon.png", self.base),
            "apps": [{
                "name": name,
                "bundleIdentifier": BUNDLE,
                "developerName": "Kiwi Dev",
                "subtitle": "A test app",
                "localizedDescription": "Description",
                "iconURL": format!("{}/icon.png", self.base),
                "tintColor": "3a3",
                "category": "utilities",
                "screenshots": [format!("{}/shot.png", self.base)],
                "appPermissions": {
                    "entitlements": ["com.apple.security.application-groups"],
                    "privacy": { "NSCameraUsageDescription": "Scan codes" }
                },
                "versions": versions
            }]
        })
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.cancel.cancel();
    }
}

/// The development certificate (DER) of the test identity; its SHA-1 is the identity's.
pub const CERT_DER: &[u8] = b"kiwi test Apple Development certificate";

pub fn identity() -> Identity {
    use sha1::{Digest, Sha1};
    Identity {
        sha1: hex::encode_upper(Sha1::digest(CERT_DER)),
        name: "Apple Development: Test (Z9Y8X7W6V5)".into(),
        team_id: Some(TEAM.into()),
        expires_at: Some("2099-01-01T00:00:00Z".parse().unwrap()),
    }
}

pub fn wildcard_profile(expires_at: DateTime<Utc>) -> Profile {
    let mut entitlements = plist::Dictionary::new();
    for key in [
        "application-identifier",
        "com.apple.developer.team-identifier",
        "get-task-allow",
        "keychain-access-groups",
    ] {
        entitlements.insert(key.into(), plist::Value::Boolean(true));
    }
    Profile {
        uuid: "9f1c2b3a-4d5e-4f60-8a7b-0c1d2e3f4a5b".into(),
        name: "iOS Team Provisioning Profile: *".into(),
        path: PathBuf::from("/profiles/wildcard.mobileprovision"),
        team_id: TEAM.into(),
        app_id_pattern: format!("{TEAM}.*"),
        created_at: expires_at - chrono::Duration::days(365),
        expires_at,
        devices: vec!["00008150-000000000000001E".into()],
        entitlements,
        certificate_sha1s: vec![identity().sha1],
        platforms: vec!["iOS".into()],
        development: true,
    }
}

/// Copies the IPA instead of re-signing it. `hold` pauses signing until released.
pub struct FakeSigner {
    pub hold: AtomicBool,
    pub signed: AtomicUsize,
    pub fail: Mutex<Option<String>>,
    pub profile_expiry: Mutex<DateTime<Utc>>,
    /// Requests the signer received.
    pub requests: Mutex<Vec<SignRequest>>,
    /// The backend it reports (this system's by default).
    pub backend: Mutex<Backend>,
}

impl Default for FakeSigner {
    fn default() -> Self {
        Self {
            hold: AtomicBool::new(false),
            signed: AtomicUsize::new(0),
            fail: Mutex::new(None),
            profile_expiry: Mutex::new(Utc::now() + chrono::Duration::days(360)),
            requests: Mutex::default(),
            backend: Mutex::new(Backend::current()),
        }
    }
}

impl Signer for FakeSigner {
    fn backend(&self) -> Backend {
        *self.backend.lock().unwrap()
    }
    fn identities(&self) -> Result<Vec<Identity>, String> {
        Ok(vec![identity()])
    }
    fn profiles(&self) -> Vec<(PathBuf, Result<Profile, String>)> {
        let p = wildcard_profile(*self.profile_expiry.lock().unwrap());
        vec![(p.path.clone(), Ok(p))]
    }
    fn sign(
        &self,
        req: &SignRequest,
        cancel: &std::sync::atomic::AtomicBool,
        progress: &dyn Fn(SignStep),
    ) -> Result<SignReport, String> {
        progress(SignStep::Extracting);
        while self.hold.load(Ordering::SeqCst) {
            if cancel.load(Ordering::SeqCst) {
                return Err("Signing was cancelled".into());
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        if let Some(e) = self.fail.lock().unwrap().clone() {
            return Err(e);
        }
        for done in 0..3 {
            progress(SignStep::Signing { done, total: 3 });
        }
        progress(SignStep::Packaging);
        std::fs::copy(&req.input_ipa, &req.output_ipa).map_err(|e| e.to_string())?;
        self.signed.fetch_add(1, Ordering::SeqCst);
        self.requests.lock().unwrap().push(req.clone());
        let profile =
            kiwi_signer::select_profile(&req.profiles, &req.identity, &req.bundle_id, Utc::now())
                .map_err(|e| e.to_string())?;
        let info = kiwi_source::inspect_ipa(&req.input_ipa, &CancellationToken::new())
            .map_err(|e| e.to_string())?;
        Ok(SignReport {
            bundle_id: req.bundle_id.clone(),
            version: info.version,
            build: info.build,
            bundles: vec![(BUNDLE.into(), req.bundle_id.clone())],
            profile_uuids: vec![profile.uuid.clone()],
            expires_at: profile.expires_at,
            original_entitlements: vec!["com.apple.security.application-groups".into()],
            dropped_entitlements: vec!["com.apple.security.application-groups".into()],
            size: std::fs::metadata(&req.output_ipa)
                .map_err(|e| e.to_string())?
                .len(),
        })
    }
}

#[derive(Default)]
pub struct FakeDevices {
    pub whois_calls: AtomicUsize,
    /// The next this many whois calls fail (e.g. tailscaled restarting).
    pub whois_errors: AtomicUsize,
    pub peers: Mutex<HashMap<IpAddr, TailscalePeer>>,
    pub cores: Mutex<Vec<CoreDevice>>,
    pub apps: Mutex<HashMap<String, Vec<DeviceApp>>>,
    /// `list_apps` waits while set; `apps_calls` counts its calls.
    pub hold_apps: Arc<AtomicBool>,
    pub apps_calls: AtomicUsize,
    /// `devicectl_available` answers false while set.
    pub no_devicectl: AtomicBool,
    /// Who owns the Mac in Tailscale.
    pub owner: Mutex<Option<TailscaleUser>>,
    /// The StableID of Kiwi's own computer in Tailscale.
    pub node: Mutex<Option<String>>,
}

/// The node of the computer Kiwi runs on (see [`FakeDevices::add_this_computer`]).
pub const THIS_NODE: &str = "nKIWIHOST";

pub fn fixture_devices(name: &str) -> Vec<CoreDevice> {
    let path = format!(
        "{}/../kiwi-device/tests/fixtures/{name}",
        env!("CARGO_MANIFEST_DIR")
    );
    kiwi_device::parse_devices(&std::fs::read(path).unwrap()).unwrap()
}

impl FakeDevices {
    pub fn with_phone() -> Self {
        let devices = Self::default();
        devices.peers.lock().unwrap().insert(
            PHONE_IP.parse().unwrap(),
            TailscalePeer {
                stable_id: "nPHONE1CNTRL".into(),
                name: "iphone194".into(),
                dns_name: "iphone194.example.ts.net".into(),
                os: Some("iOS".into()),
                user_login: Some("alex@example.com".into()),
                user_name: None,
            },
        );
        *devices.cores.lock().unwrap() = fixture_devices("devicectl_list_devices_simulators.json");
        *devices.owner.lock().unwrap() = Some(TailscaleUser {
            login: "alex@example.com".into(),
            name: Some("Alex Morgan".into()),
        });
        *devices.node.lock().unwrap() = Some(THIS_NODE.into());
        devices
    }

    /// The owner's desktop computer at `ip`, with Tailscale's `os` (`linux`, `windows`, ...).
    pub fn add_computer(&self, ip: &str, os: &str) {
        self.peers.lock().unwrap().insert(
            ip.parse().unwrap(),
            TailscalePeer {
                stable_id: "nDESKTOP1CNTRL".into(),
                name: "desktop".into(),
                dns_name: "desktop.example.ts.net".into(),
                os: Some(os.into()),
                user_login: Some("alex@example.com".into()),
                user_name: None,
            },
        );
    }

    /// Kiwi's own computer at `ip`, which belongs to the owner, as whois reports it for a
    /// request made on it through its tailnet address.
    pub fn add_this_computer(&self, ip: &str) {
        self.peers.lock().unwrap().insert(
            ip.parse().unwrap(),
            TailscalePeer {
                stable_id: THIS_NODE.into(),
                name: "kiwi-server".into(),
                dns_name: "kiwi-server.example.ts.net".into(),
                os: Some("linux".into()),
                user_login: Some("alex@example.com".into()),
                user_name: None,
            },
        );
    }

    /// Adds another Tailscale user's iPhone at `ip`.
    pub fn add_peer(&self, ip: &str, stable_id: &str, login: &str, name: &str) {
        self.peers.lock().unwrap().insert(
            ip.parse().unwrap(),
            TailscalePeer {
                stable_id: stable_id.into(),
                name: format!("{}-iphone", name.to_lowercase()),
                dns_name: format!("{}-iphone.example.ts.net", name.to_lowercase()),
                os: Some("iOS".into()),
                user_login: Some(login.into()),
                user_name: Some(name.into()),
            },
        );
    }
}

impl FakeDevices {
    /// Adds the owner's Android phone at `ip`.
    pub fn add_android(&self, ip: &str) {
        self.peers.lock().unwrap().insert(
            ip.parse().unwrap(),
            TailscalePeer {
                stable_id: "nANDROID1CNTRL".into(),
                name: "pixel-9a".into(),
                dns_name: "pixel-9a.example.ts.net".into(),
                os: Some("android".into()),
                user_login: Some("alex@example.com".into()),
                user_name: None,
            },
        );
    }
}

impl DeviceTools for FakeDevices {
    fn whois(&self, ip: IpAddr) -> BoxFuture<'_, Result<Option<TailscalePeer>, String>> {
        self.whois_calls.fetch_add(1, Ordering::SeqCst);
        // Tests look devices up one at a time.
        let fail = self.whois_errors.load(Ordering::SeqCst) > 0;
        if fail {
            self.whois_errors.fetch_sub(1, Ordering::SeqCst);
        }
        let peer = self.peers.lock().unwrap().get(&ip).cloned();
        Box::pin(async move {
            if fail {
                return Err("tailscale whois timed out".to_owned());
            }
            Ok(peer)
        })
    }
    fn owner(&self) -> BoxFuture<'_, Result<Option<TailscaleUser>, String>> {
        let owner = self.owner.lock().unwrap().clone();
        Box::pin(async move { Ok(owner) })
    }
    fn node_id(&self) -> BoxFuture<'_, Result<Option<String>, String>> {
        let node = self.node.lock().unwrap().clone();
        Box::pin(async move { Ok(node) })
    }
    fn devicectl_available(&self) -> BoxFuture<'_, bool> {
        let available = !self.no_devicectl.load(Ordering::SeqCst);
        Box::pin(async move { available })
    }
    fn list_devices(&self) -> BoxFuture<'_, Result<Vec<CoreDevice>, String>> {
        let cores = self.cores.lock().unwrap().clone();
        Box::pin(async move { Ok(cores) })
    }
    fn list_apps<'a>(&'a self, device: &'a str) -> BoxFuture<'a, Result<Vec<DeviceApp>, String>> {
        self.apps_calls.fetch_add(1, Ordering::SeqCst);
        let hold = self.hold_apps.clone();
        Box::pin(async move {
            while hold.load(Ordering::SeqCst) {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
            let apps = self.apps.lock().unwrap().get(device).cloned();
            apps.ok_or_else(|| "device not found".to_owned())
        })
    }
}

/// The user of each loopback connection to Kiwi, as `/proc/net/tcp` would show it: `uid`
/// (tailscaled's, root, unless a test changes it) for every connection.
pub struct FakePeers {
    pub uid: Mutex<Result<Option<u32>, String>>,
    /// The connections looked up: (client, Kiwi's end).
    pub lookups: Mutex<Vec<(SocketAddr, SocketAddr)>>,
}

impl Default for FakePeers {
    fn default() -> Self {
        Self {
            uid: Mutex::new(Ok(Some(0))),
            lookups: Mutex::default(),
        }
    }
}

impl FakePeers {
    pub fn set(&self, uid: Result<Option<u32>, &str>) {
        *self.uid.lock().unwrap() = uid.map_err(str::to_owned);
    }
}

impl PeerUids for FakePeers {
    fn uid(&self, client: SocketAddr, server: SocketAddr) -> Result<Option<u32>, String> {
        self.lookups.lock().unwrap().push((client, server));
        self.uid.lock().unwrap().clone()
    }
}

/// Records every push; answers with queued `outcomes`, then `Delivered`.
#[derive(Default)]
pub struct FakePush {
    pub sent: Mutex<Vec<(String, Value)>>,
    pub outcomes: Mutex<VecDeque<Result<Outcome, SendError>>>,
    /// The VAPID subjects Kiwi set after its start.
    pub subjects: Mutex<Vec<String>>,
}

impl FakePush {
    pub fn answer(&self, outcome: Result<Outcome, SendError>, times: usize) {
        let mut outcomes = self.outcomes.lock().unwrap();
        outcomes.extend(std::iter::repeat_n(outcome, times));
    }

    /// Deliveries recorded so far (attempts, successful or not).
    pub fn attempts(&self) -> usize {
        self.sent.lock().unwrap().len()
    }
}

impl PushTransport for FakePush {
    fn send<'a>(
        &'a self,
        sub: &'a Subscription,
        payload: &'a [u8],
        _topic: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Outcome, SendError>> {
        let value: Value = serde_json::from_slice(payload).unwrap();
        self.sent
            .lock()
            .unwrap()
            .push((sub.endpoint.clone(), value));
        let outcome = self
            .outcomes
            .lock()
            .unwrap()
            .pop_front()
            .unwrap_or(Ok(Outcome::Delivered));
        Box::pin(async move { outcome })
    }

    fn set_subject(&self, subject: &str) {
        self.subjects.lock().unwrap().push(subject.to_owned());
    }
}

pub fn web_dir(root: &Path) -> PathBuf {
    let web = root.join("web");
    std::fs::create_dir_all(web.join("assets")).unwrap();
    std::fs::write(web.join("index.html"), "<!doctype html><title>Kiwi</title>").unwrap();
    std::fs::write(web.join("sw.js"), "// sw").unwrap();
    std::fs::write(web.join("assets/index-abc123.js"), "console.log(1)").unwrap();
    std::fs::write(web.join("manifest.webmanifest"), "{}").unwrap();
    web
}

/// How a [`TestKiwi`] is set up.
pub struct Setup {
    pub devices: FakeDevices,
    pub allow_simulators: bool,
    pub app_store_url: String,
    /// Instead of the [`FakeSigner`] (for example the real one).
    pub signer: Option<Arc<dyn Signer>>,
    /// Who may forward tailnet addresses: tailscaled (uid 0, see [`FakePeers`]) as on
    /// Linux, unless a test runs Kiwi as on a Mac.
    pub local: LocalTrust,
    pub public_url: Option<String>,
}

impl Default for Setup {
    fn default() -> Self {
        Self {
            devices: FakeDevices::with_phone(),
            allow_simulators: false,
            // A closed port: tests never reach the real App Store.
            app_store_url: "http://127.0.0.1:9".into(),
            signer: None,
            local: LocalTrust::ProxyUid(0),
            public_url: Some("https://kiwi.test".into()),
        }
    }
}

/// A running Kiwi with fakes.
pub struct TestKiwi {
    pub kiwi: Arc<Kiwi>,
    pub base: String,
    pub signer: Arc<FakeSigner>,
    pub devices: Arc<FakeDevices>,
    pub peers: Arc<FakePeers>,
    pub push: Arc<FakePush>,
    pub dir: tempfile::TempDir,
    http: reqwest::Client,
}

impl TestKiwi {
    pub async fn start() -> Self {
        Self::start_in(
            tempfile::tempdir().unwrap(),
            FakeDevices::with_phone(),
            false,
        )
        .await
    }

    /// A Kiwi that already built YouTube before Kiwi Builds became a setting: it keeps
    /// building, with YouTube added.
    pub async fn start_building() -> Self {
        let dir = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(dir.path().join("data")).unwrap();
        let db = kiwi_server::db::Db::open(&dir.path().join("data/kiwi.db")).unwrap();
        let youtube = kiwi_server::automation::recipe("youtube").unwrap();
        let state = serde_json::json!({"apps": {"youtube": {
            "added": true, "preferences": youtube.default_preferences(),
        }}});
        db.set_meta("automation_v2", &state.to_string()).unwrap();
        drop(db);
        Self::start_in(dir, FakeDevices::with_phone(), false).await
    }

    pub async fn start_in(
        dir: tempfile::TempDir,
        devices: FakeDevices,
        allow_simulators: bool,
    ) -> Self {
        // A closed port: tests never reach the real App Store.
        Self::start_with(dir, devices, allow_simulators, "http://127.0.0.1:9").await
    }

    /// With the App Store lookups served by a [`Fixture`] (its `base`).
    pub async fn start_with_store(app_store_url: &str) -> Self {
        Self::start_with(
            tempfile::tempdir().unwrap(),
            FakeDevices::with_phone(),
            false,
            app_store_url,
        )
        .await
    }

    pub async fn start_with(
        dir: tempfile::TempDir,
        devices: FakeDevices,
        allow_simulators: bool,
        app_store_url: &str,
    ) -> Self {
        let setup = Setup {
            devices,
            allow_simulators,
            app_store_url: app_store_url.into(),
            ..Setup::default()
        };
        Self::start_setup(dir, setup).await
    }

    /// With `signer` (for example the real one) instead of the [`FakeSigner`].
    pub async fn start_with_signer(dir: tempfile::TempDir, signer: Arc<dyn Signer>) -> Self {
        let setup = Setup {
            signer: Some(signer),
            ..Setup::default()
        };
        Self::start_setup(dir, setup).await
    }

    /// Kiwi as on a Mac (or with `--trust-local`), which is its owner's alone: a browser on
    /// it is the owner's.
    pub async fn start_on_mac(dir: tempfile::TempDir, devices: FakeDevices) -> Self {
        let setup = Setup {
            devices,
            local: LocalTrust::Anyone,
            ..Setup::default()
        };
        Self::start_setup(dir, setup).await
    }

    pub async fn start_setup(dir: tempfile::TempDir, setup: Setup) -> Self {
        let signer = Arc::new(FakeSigner::default());
        let devices = Arc::new(setup.devices);
        let peers = Arc::new(FakePeers::default());
        let push = Arc::new(FakePush::default());
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let listen = listener.local_addr().unwrap();
        let kiwi = Kiwi::new(Options {
            config: Config {
                listen,
                data_dir: dir.path().join("data"),
                web_dir: web_dir(dir.path()),
                public_url: setup.public_url,
                allow_simulators: setup.allow_simulators,
                owner: None,
                country: None,
                // The fake runners only need a Python 3 on PATH (CI's macOS runner has no
                // Homebrew Python at config::PYTHON).
                python: "python3".into(),
                local: setup.local,
            },
            source_client: ClientOptions {
                read_timeout: Duration::from_secs(10),
                allow_local: true,
            },
            signer: setup.signer.unwrap_or_else(|| signer.clone()),
            devices: devices.clone(),
            peers: peers.clone(),
            push: Some(push.clone()),
            app_store_url: setup.app_store_url,
        })
        .await
        .unwrap();
        let base = format!("http://{listen}");
        let app = kiwi_server::app(kiwi.clone());
        let stop = kiwi.cancel.clone();
        tokio::spawn(async move {
            axum::serve(listener, app)
                .with_graceful_shutdown(stop.cancelled_owned())
                .await
                .unwrap();
        });
        Self {
            kiwi,
            base,
            signer,
            devices,
            peers,
            push,
            dir,
            http: reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
        }
    }

    /// The phone, as proxied by `tailscale serve`.
    pub fn phone(&self) -> Api {
        Api {
            http: self.http.clone(),
            base: self.base.clone(),
            forwarded_for: Some(PHONE_IP.into()),
            cookie: None,
            user_agent: Some(
                "Mozilla/5.0 (iPhone; CPU iPhone OS 18_6 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/27.0 Mobile/15E148 Safari/604.1".into(),
            ),
        }
    }

    /// Another tailnet device, as proxied by `tailscale serve` (see [`FakeDevices::add_peer`]).
    pub fn device(&self, ip: &str) -> Api {
        Api {
            forwarded_for: Some(ip.into()),
            ..self.phone()
        }
    }

    /// The owner's Android phone (see [`FakeDevices::add_android`]), in Chrome.
    pub fn android(&self, ip: &str) -> Api {
        Api {
            forwarded_for: Some(ip.into()),
            user_agent: Some(
                "Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Mobile Safari/537.36".into(),
            ),
            ..self.phone()
        }
    }

    /// A browser on the computer Kiwi runs on, without Tailscale identity: the owner's on a
    /// Mac ([`TestKiwi::start_on_mac`]), nobody's elsewhere.
    pub fn browser(&self) -> Api {
        Api {
            http: self.http.clone(),
            base: self.base.clone(),
            forwarded_for: None,
            cookie: None,
            user_agent: None,
        }
    }

    pub fn raw(&self) -> &reqwest::Client {
        &self.http
    }

    /// Stops the server and background jobs, keeping the data directory.
    pub async fn stop(mut self) -> tempfile::TempDir {
        self.kiwi.shutdown().await;
        std::mem::replace(&mut self.dir, tempfile::tempdir().unwrap())
    }
}

impl Drop for TestKiwi {
    fn drop(&mut self) {
        self.kiwi.cancel.cancel();
    }
}

#[derive(Clone)]
pub struct Api {
    pub http: reqwest::Client,
    pub base: String,
    pub forwarded_for: Option<String>,
    pub cookie: Option<String>,
    pub user_agent: Option<String>,
}

pub struct Reply {
    pub status: StatusCode,
    pub headers: reqwest::header::HeaderMap,
    pub body: Value,
}

impl Api {
    pub fn request(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        let mut r = self.http.request(method, format!("{}{path}", self.base));
        if let Some(ip) = &self.forwarded_for {
            r = r.header("X-Forwarded-For", ip);
        }
        if let Some(c) = &self.cookie {
            r = r.header("Cookie", c);
        }
        if let Some(ua) = &self.user_agent {
            r = r.header("User-Agent", ua);
        }
        r
    }

    async fn reply(r: reqwest::RequestBuilder) -> Reply {
        let response = r.send().await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = response.bytes().await.unwrap();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into()))
        };
        Reply {
            status,
            headers,
            body,
        }
    }

    pub async fn get(&self, path: &str) -> Reply {
        Self::reply(self.request(Method::GET, path)).await
    }

    /// A mutation as the web app sends it (`X-Kiwi: 1`, JSON body; none for DELETE).
    pub async fn send(&self, method: Method, path: &str, body: Option<Value>) -> Reply {
        let mut r = self
            .request(method.clone(), path)
            .header("X-Kiwi", "1")
            .header("Content-Type", "application/json");
        if method != Method::DELETE {
            r = r.body(body.unwrap_or_else(|| json!({})).to_string());
        }
        Self::reply(r).await
    }

    pub async fn post(&self, path: &str, body: Value) -> Reply {
        self.send(Method::POST, path, Some(body)).await
    }

    pub async fn ok(&self, path: &str) -> Value {
        let r = self.get(path).await;
        assert_eq!(r.status, StatusCode::OK, "GET {path}: {}", r.body);
        r.body
    }
}

/// Adds the fixture source and returns its id.
pub async fn add_source(api: &Api, fixture: &Fixture) -> String {
    let r = api
        .post("/api/sources", json!({ "url": fixture.feed_url() }))
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    r.body["id"].as_str().unwrap().to_owned()
}

/// Polls a build until it is ready or failed.
pub async fn wait_build(api: &Api, id: &str) -> Value {
    for _ in 0..400 {
        let b = api.ok(&format!("/api/builds/{id}")).await;
        if matches!(b["status"].as_str(), Some("ready" | "failed")) {
            return b;
        }
        tokio::time::sleep(Duration::from_millis(25)).await;
    }
    panic!("build {id} did not finish");
}

pub async fn app_state(api: &Api, source: &str) -> Value {
    api.ok(&format!("/api/apps/{source}/{BUNDLE}")).await["state"].clone()
}

/// The `/ota/<token>/...` path of an install URL.
pub fn ota_path(install_url: &str) -> String {
    let encoded = install_url
        .strip_prefix("itms-services://?action=download-manifest&url=")
        .unwrap();
    let manifest: String = url::form_urlencoded::parse(format!("u={encoded}").as_bytes())
        .next()
        .unwrap()
        .1
        .into_owned();
    let path = manifest.strip_prefix("https://kiwi.test").unwrap();
    path.strip_suffix("/manifest.plist").unwrap().to_owned()
}

/// Prepares the app of `source`, downloads its IPA like the OTA installer and waits until
/// the install is recorded. Returns the build.
pub async fn install(k: &TestKiwi, api: &Api, source: &str) -> Value {
    let prepared = api
        .post(&format!("/api/apps/{source}/{BUNDLE}/prepare"), json!({}))
        .await;
    assert_eq!(prepared.status, StatusCode::ACCEPTED, "{}", prepared.body);
    let build = wait_build(api, prepared.body["id"].as_str().unwrap()).await;
    assert_eq!(build["status"], "ready", "{build}");
    let ota = ota_path(build["install_url"].as_str().unwrap());
    let response = k
        .raw()
        .get(format!("{}{ota}/app.ipa", k.base))
        .header("User-Agent", INSTALLER_UA)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.bytes().await.unwrap();
    for _ in 0..100 {
        if app_state(api, source).await["action"] == "open" {
            return build;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("the install from {source} was not recorded");
}

/// Waits until `cond` holds (polling every 10 ms, at most 5 s).
pub async fn wait_until(mut cond: impl FnMut() -> bool) {
    for _ in 0..500 {
        if cond() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!("condition not reached");
}
