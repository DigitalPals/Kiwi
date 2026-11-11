//! The Mac builder (docs/api.md "Mac builder"): pairing, the builder's protocol, and iOS
//! builds of a Kiwi that doesn't run on macOS made by `kiwi builder run`'s loop, running
//! in-process with a fake runner against a test Kiwi.
mod common;

use std::{
    io::{Cursor, Write},
    path::{Path, PathBuf},
    sync::{Arc, atomic::Ordering},
    time::Duration,
};

use common::*;
use kiwi_server::mac_builder::{
    Timing,
    client::{self, Builder, Saved},
};
use kiwi_source::CancellationToken;
use reqwest::{Method, StatusCode, header::HeaderMap};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const MAC_IP: &str = "100.64.0.20";
const OTHER_IP: &str = "100.64.0.30";
const NAME: &str = "Test Mac";

/// Short timings, and Kiwi as on Linux: iOS apps need a Mac builder.
fn not_a_mac(k: &TestKiwi) {
    let mac = &k.kiwi.automation.mac;
    mac.supported.store(true, Ordering::SeqCst);
    *mac.timing.write().unwrap() = Timing {
        online: Duration::from_millis(1500),
        lost: Duration::from_millis(1500),
        poll: Duration::from_millis(300),
        ..Timing::default()
    };
    k.kiwi.automation.builds_ios.store(false, Ordering::SeqCst);
}

/// A Kiwi on Linux started with `--trust-local`: the computer is the owner's, so a builder
/// may call it on loopback (as through an SSH tunnel). Kiwi Builds is on.
async fn kiwi() -> TestKiwi {
    let k = TestKiwi::start_on_mac(tempfile::tempdir().unwrap(), FakeDevices::with_phone()).await;
    not_a_mac(&k);
    builds_on(&k).await;
    fake_ios_runner(&k);
    k
}

async fn builds_on(k: &TestKiwi) {
    let on = k
        .phone()
        .send(
            Method::PATCH,
            "/api/settings",
            Some(json!({"kiwi_builds": true})),
        )
        .await;
    assert_eq!(on.status, StatusCode::OK, "{}", on.body);
}

/// The Mac's runner: like runner.py's events, with an IPA and its provenance for a build.
/// `automation/pause-<command>` holds it; SIGTERM leaves `automation/terminated`.
fn fake_ios_runner(k: &TestKiwi) -> PathBuf {
    let script = k.dir.path().join("fake-ios-runner.py");
    std::fs::write(
        &script,
        r#"
import sys,json,pathlib,zipfile,plistlib,hashlib,time,signal
args=sys.argv
command=args[1]
root=pathlib.Path(args[args.index('--data-dir')+1])/'automation'
root.mkdir(parents=True,exist_ok=True)
def stopped(*_):
    (root/'terminated').write_text('1')
    sys.exit(1)
signal.signal(signal.SIGTERM,stopped)
request=json.loads(pathlib.Path(args[args.index('--request')+1]).read_text())
def emit(kind,**data):print(json.dumps(dict(event=kind,**data)),flush=True)
def pause(name):
    while (root/('pause-'+name)).exists():time.sleep(0.05)
if command=='doctor':
    emit('health',checks=[dict(name='Mac build tools',ready=True),dict(name='App Store sign-in',ready=(root/'signed-in').exists())],ready=True)
    sys.exit(0)
recipe=args[args.index('--recipe')+1]
bundle=dict(youtube='com.google.ios.youtube',twitch='tv.twitch')[recipe]
version='21.44.1'
if command=='plan':
    emit('health',checks=[dict(name='Mac build tools',ready=True)],ready=True)
    emit('stage',stage='Checking the App Store')
    pause('plan')
    emit('plan',plan=dict(key='a'*64,version=version,external_id='1234',tweaks=request['tweaks'],name=request.get('name')))
else:
    emit('stage',stage='Decrypting YouTube')
    pause('build')
    p=root/'candidate.ipa'
    info=dict(CFBundleIdentifier=bundle,CFBundleShortVersionString=version,CFBundleVersion=version,KiwiBuildRevision=request['sequence'],MinimumOSVersion='17.0',CFBundleSupportedPlatforms=['iPhoneOS'],UIDeviceFamily=[1])
    with zipfile.ZipFile(p,'w') as z:
        z.writestr('Payload/YouTube.app/Info.plist',plistlib.dumps(info))
        z.writestr('Payload/YouTube.app/YouTube',b'x'*300000)
    sha=hashlib.sha256(p.read_bytes()).hexdigest()
    dest=root/'artifacts'
    dest.mkdir(exist_ok=True)
    p.rename(dest/(sha+'.ipa'))
    (dest/(sha+'.json')).write_text(json.dumps(dict(sha256=sha,plan=request['plan'],sequence=request['sequence'])))
    emit('artifact',sha256=sha,version=version,build=version,revision=request['sequence'],min_os='17.0',size=(dest/(sha+'.ipa')).stat().st_size,notes='Fixture')
"#,
    )
    .unwrap();
    script
}

/// Android builds, to show they don't wait for the Mac.
fn fake_android_runner(k: &TestKiwi) {
    let script = k.dir.path().join("fake-android-runner.py");
    std::fs::write(
        &script,
        r#"
import sys,json,pathlib,hashlib
command=sys.argv[1]
recipe=sys.argv[sys.argv.index('--recipe')+1]
root=pathlib.Path(sys.argv[sys.argv.index('--data-dir')+1])/'automation'
request=json.loads(pathlib.Path(sys.argv[sys.argv.index('--request')+1]).read_text())
def emit(kind,**data):print(json.dumps(dict(event=kind,**data)),flush=True)
if command=='plan':
 emit('plan',plan=dict(app=recipe,platform='android',key=hashlib.sha256(recipe.encode()).hexdigest(),version='21.30.216',external_id='1',tweaks=[],name=None))
else:
 data=b'PK'+recipe.encode()+bytes(str(request['sequence']),'ascii')+b'x'*300000
 sha=hashlib.sha256(data).hexdigest()
 dest=root/'artifacts'
 dest.mkdir(exist_ok=True)
 (dest/(sha+'.apk')).write_bytes(data)
 emit('artifact',sha256=sha,version='21.30.216',build='1',revision=request['sequence'],min_os=None,size=len(data),package='app.morphe.android.youtube',signer='c'*64,notes='Fixture')
"#,
    )
    .unwrap();
    *k.kiwi.automation.android_runner.write().unwrap() = script;
}

/// A pairing code from Settings.
async fn code(k: &TestKiwi) -> String {
    let r = k.phone().post("/api/builder/pairing", json!({})).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    r.body["code"].as_str().unwrap().to_owned()
}

/// A call as the builder makes it (`X-Kiwi`, JSON, the token), from `forwarded_for`.
async fn call(
    k: &TestKiwi,
    method: Method,
    path: &str,
    token: Option<&str>,
    forwarded_for: Option<&str>,
    body: Value,
) -> Reply {
    let mut r = k
        .raw()
        .request(method, format!("{}{path}", k.base))
        .header("X-Kiwi", "1")
        .json(&body);
    if let Some(token) = token {
        r = r.bearer_auth(token);
    }
    if let Some(ip) = forwarded_for {
        r = r.header("X-Forwarded-For", ip);
    }
    reply(r).await
}

async fn reply(r: reqwest::RequestBuilder) -> Reply {
    let response = r.send().await.unwrap();
    let status = response.status();
    let headers = response.headers().clone();
    let bytes = response.bytes().await.unwrap();
    let body = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    Reply {
        status,
        headers,
        body,
    }
}

fn hello() -> Value {
    json!({"version": env!("CARGO_PKG_VERSION"), "os": "macOS 26.1 (arm64)",
        "runner_sha256": *kiwi_server::automation::RUNNER_SHA256,
        "recipes_sha256": *kiwi_server::automation::RECIPES_SHA256})
}

/// Pairs on loopback and returns the token.
async fn pair_raw(k: &TestKiwi) -> String {
    let code = code(k).await;
    let mut body = hello();
    body["code"] = json!(code);
    body["name"] = json!(NAME);
    let r = call(k, Method::POST, "/api/builder/pair", None, None, body).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    r.body["token"].as_str().unwrap().to_owned()
}

/// Polls until a task with `command` comes; doctor checks on the way are finished.
async fn task(k: &TestKiwi, token: &str, command: &str) -> Value {
    let mut body = hello();
    body["name"] = json!(NAME);
    for _ in 0..50 {
        let r = call(
            k,
            Method::POST,
            "/api/builder/poll",
            Some(token),
            None,
            body.clone(),
        )
        .await;
        if r.status == StatusCode::NO_CONTENT {
            continue;
        }
        assert_eq!(r.status, StatusCode::OK, "{}", r.body);
        let task = r.body["task"].clone();
        if task["command"] == command {
            return task;
        }
        assert_eq!(task["command"], "doctor", "{task}");
        let id = task["id"].as_str().unwrap();
        let path = format!("/api/builder/tasks/{id}/finish");
        let done = call(
            k,
            Method::POST,
            &path,
            Some(token),
            None,
            json!({"status": "ok"}),
        )
        .await;
        assert_eq!(done.status, StatusCode::NO_CONTENT, "{}", done.body);
    }
    panic!("no {command} task");
}

async fn events(k: &TestKiwi, token: &str, id: &str, events: Value) -> Reply {
    let path = format!("/api/builder/tasks/{id}/events");
    call(
        k,
        Method::POST,
        &path,
        Some(token),
        None,
        json!({ "events": events }),
    )
    .await
}

async fn finish(k: &TestKiwi, token: &str, id: &str, body: Value) -> Reply {
    let path = format!("/api/builder/tasks/{id}/finish");
    call(k, Method::POST, &path, Some(token), None, body).await
}

async fn upload(k: &TestKiwi, token: &str, id: &str, sha: &str, bytes: Vec<u8>) -> Reply {
    let r = k
        .raw()
        .put(format!(
            "{}/api/builder/tasks/{id}/artifact?sha256={sha}",
            k.base
        ))
        .header("X-Kiwi", "1")
        .header("Content-Type", "application/octet-stream")
        .bearer_auth(token)
        .body(bytes);
    reply(r).await
}

/// The newest job of `app` once `done` holds for it.
async fn job(
    k: &TestKiwi,
    app: &str,
    done: impl Fn(&kiwi_server::automation::Job) -> bool,
) -> kiwi_server::automation::Job {
    for _ in 0..1000 {
        if let Some(job) = k
            .kiwi
            .automation
            .snapshot()
            .jobs
            .into_iter()
            .find(|j| j.app == app)
            .filter(|j| done(j))
        {
            return job;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    panic!(
        "{app}: {:?}",
        k.kiwi
            .automation
            .snapshot()
            .jobs
            .iter()
            .find(|j| j.app == app)
    );
}

async fn finished(k: &TestKiwi, app: &str) -> kiwi_server::automation::Job {
    job(k, app, |j| j.finished_at.is_some()).await
}

async fn add(k: &TestKiwi, app: &str) -> Reply {
    let phone = k.phone();
    let preferences = phone.ok(&format!("/api/automation/{app}")).await["preferences"].clone();
    phone
        .post(&format!("/api/automation/{app}/add"), preferences)
        .await
}

/// `kiwi builder run`'s loop, in-process.
struct Mac {
    data: PathBuf,
    stop: CancellationToken,
    handle: tokio::task::JoinHandle<()>,
}

impl Mac {
    async fn pair(k: &TestKiwi) -> (Self, Saved) {
        let data = k.dir.path().join("Kiwi Builder");
        let server = client::server_url(&k.base).unwrap();
        let code = code(k).await;
        // As typed: case and dashes don't matter.
        let typed = code.replace('-', "").to_lowercase();
        let saved = client::pair(
            &client::http().unwrap(),
            &server,
            &typed,
            NAME,
            &HeaderMap::new(),
        )
        .await
        .unwrap();
        client::save(&data, &saved).unwrap();
        (Self::start(k, &data, saved.clone(), |_| {}), saved)
    }

    fn start(k: &TestKiwi, data: &Path, saved: Saved, change: impl FnOnce(&mut Builder)) -> Self {
        let mut builder = Builder::new(data.to_owned(), "python3".into(), saved).unwrap();
        builder.runner = k.dir.path().join("fake-ios-runner.py");
        builder.heartbeat = Duration::from_millis(200);
        change(&mut builder);
        let builder = Arc::new(builder);
        let stop = CancellationToken::new();
        let handle = tokio::spawn({
            let stop = stop.clone();
            async move { builder.run(&stop).await }
        });
        Self {
            data: data.to_owned(),
            stop,
            handle,
        }
    }

    fn automation(&self) -> PathBuf {
        let root = self.data.join("automation");
        std::fs::create_dir_all(&root).unwrap();
        root
    }

    async fn stop(self) {
        self.stop.cancel();
        self.handle.await.unwrap();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pairing_codes_are_one_time_expire_and_are_rate_limited() {
    let k = kiwi().await;
    let phone = k.phone();
    let none = phone.ok("/api/builder").await;
    assert_eq!(
        none,
        json!({"supported": true, "reason": null, "builder": null, "pairing": null})
    );

    let made = phone.post("/api/builder/pairing", json!({})).await;
    assert_eq!(made.status, StatusCode::CREATED, "{}", made.body);
    let code = made.body["code"].as_str().unwrap().to_owned();
    assert_eq!(code.len(), 14, "{code}");
    assert!(
        code.split('-').all(|part| part.len() == 4
            && part
                .bytes()
                .all(|b| b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ".contains(&b))),
        "{code}"
    );
    assert_eq!(
        made.body["command"],
        format!("./install.sh --builder --server https://kiwi.test --pair {code}")
    );
    assert_eq!(phone.ok("/api/builder").await["pairing"], made.body);
    // A new code replaces the unused one.
    let replaced = code.clone();
    let code = code_of(&phone.post("/api/builder/pairing", json!({})).await);
    assert_ne!(code, replaced);

    let pair = |code: &str| {
        let mut body = hello();
        body["code"] = json!(code);
        body["name"] = json!(NAME);
        body
    };
    let r = call(
        &k,
        Method::POST,
        "/api/builder/pair",
        None,
        None,
        pair(&replaced),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.body["error"]["code"], "bad_code");

    let r = call(
        &k,
        Method::POST,
        "/api/builder/pair",
        None,
        None,
        pair(&code.to_lowercase()),
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    let token = r.body["token"].as_str().unwrap();
    assert!(token.len() >= 43, "{token}");
    assert_eq!(r.body["kiwi"], "https://kiwi.test");
    assert_eq!(r.body["name"], NAME);
    // Kiwi keeps only the token's digest.
    let saved = k.kiwi.db.meta("mac_builder").unwrap().unwrap();
    assert!(!saved.contains(token));
    assert!(saved.contains(&hex::encode(Sha256::digest(token.as_bytes()))));

    let view = phone.ok("/api/builder").await;
    assert_eq!(view["pairing"], Value::Null);
    let builder = &view["builder"];
    assert_eq!(builder["name"], NAME);
    assert_eq!(builder["online"], true);
    assert_eq!(builder["compatible"], true);
    assert_eq!(builder["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(builder["os"], "macOS 26.1 (arm64)");
    assert_eq!(builder["task"], Value::Null);
    assert!(builder["paired_at"].is_string() && builder["last_seen"].is_string());
    assert_eq!(
        builder["checks"][0],
        json!({"name": "Mac builder", "ready": true})
    );
    assert_eq!(phone.ok("/api/me").await["host"]["builds_ios"], true);

    // A code works once.
    let r = call(
        &k,
        Method::POST,
        "/api/builder/pair",
        None,
        None,
        pair(&code),
    )
    .await;
    assert_eq!(r.body["error"]["code"], "bad_code");
    // An expired one doesn't.
    k.kiwi.automation.mac.timing.write().unwrap().code_lifetime = chrono::Duration::seconds(-1);
    let expired = code_of(&phone.post("/api/builder/pairing", json!({})).await);
    assert_eq!(phone.ok("/api/builder").await["pairing"], Value::Null);
    let r = call(
        &k,
        Method::POST,
        "/api/builder/pair",
        None,
        None,
        pair(&expired),
    )
    .await;
    assert_eq!(r.body["error"]["code"], "bad_code");
    k.kiwi.automation.mac.timing.write().unwrap().code_lifetime = chrono::Duration::minutes(10);
    // Nor a withdrawn one.
    let withdrawn = code_of(&phone.post("/api/builder/pairing", json!({})).await);
    let r = phone
        .send(Method::DELETE, "/api/builder/pairing", None)
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let r = call(
        &k,
        Method::POST,
        "/api/builder/pair",
        None,
        None,
        pair(&withdrawn),
    )
    .await;
    assert_eq!(r.body["error"]["code"], "bad_code");

    // Five wrong codes in ten minutes: then even the right one is refused.
    let right = code_of(&phone.post("/api/builder/pairing", json!({})).await);
    for _ in 0..2 {
        let r = call(
            &k,
            Method::POST,
            "/api/builder/pair",
            None,
            None,
            pair("AAAA-BBBB-CCCC"),
        )
        .await;
        assert_eq!(r.body["error"]["code"], "bad_code");
    }
    let r = call(
        &k,
        Method::POST,
        "/api/builder/pair",
        None,
        None,
        pair(&right),
    )
    .await;
    assert_eq!(r.status, StatusCode::TOO_MANY_REQUESTS, "{}", r.body);
    assert_eq!(r.body["error"]["code"], "rate_limited");

    // A Kiwi on macOS builds iOS apps itself.
    k.kiwi
        .automation
        .mac
        .supported
        .store(false, Ordering::SeqCst);
    assert_eq!(
        phone.ok("/api/builder").await,
        json!({"supported": false,
            "reason": "This Kiwi runs on a Mac and builds iOS apps itself.",
            "builder": null, "pairing": null})
    );
    let r = phone.post("/api/builder/pairing", json!({})).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    assert_eq!(r.body["error"]["code"], "builder_unsupported");
    let r = call(
        &k,
        Method::POST,
        "/api/builder/pair",
        None,
        None,
        pair(&right),
    )
    .await;
    assert_eq!(r.body["error"]["code"], "builder_unsupported");
    let r = call(
        &k,
        Method::POST,
        "/api/builder/poll",
        Some(token),
        None,
        hello(),
    )
    .await;
    assert_eq!(r.body["error"]["code"], "builder_unsupported");
}

fn code_of(r: &Reply) -> String {
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    r.body["code"].as_str().unwrap().to_owned()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn builder_routes_need_the_token_and_the_owners_tailnet_identity() {
    // Kiwi on Linux without --trust-local: only tailscaled may forward tailnet addresses.
    let k = TestKiwi::start().await;
    not_a_mac(&k);
    k.devices.add_computer(MAC_IP, "macOS");
    k.devices
        .add_peer(OTHER_IP, "nSAM1CNTRL", "sam@example.com", "Sam");
    k.devices.add_this_computer("100.64.0.99");
    let code = code(&k).await;
    let devices = k.kiwi.db.devices().unwrap().len();
    let mut body = hello();
    body["code"] = json!(code);
    body["name"] = json!(NAME);
    for from in [None, Some(OTHER_IP), Some("100.64.0.99")] {
        let r = call(
            &k,
            Method::POST,
            "/api/builder/pair",
            None,
            from,
            body.clone(),
        )
        .await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{from:?}: {}", r.body);
        assert_eq!(r.body["error"]["code"], "not_owner");
    }
    let r = call(
        &k,
        Method::POST,
        "/api/builder/pair",
        None,
        Some(MAC_IP),
        body,
    )
    .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    let token = r.body["token"].as_str().unwrap().to_owned();

    let poll = hello();
    for bad in [None, Some("x".repeat(43))] {
        let r = call(
            &k,
            Method::POST,
            "/api/builder/poll",
            bad.as_deref(),
            Some(MAC_IP),
            poll.clone(),
        )
        .await;
        assert_eq!(r.status, StatusCode::UNAUTHORIZED, "{}", r.body);
        assert_eq!(r.body["error"]["code"], "builder_unpaired");
    }
    let r = call(
        &k,
        Method::POST,
        "/api/builder/poll",
        Some(&token),
        Some(OTHER_IP),
        poll.clone(),
    )
    .await;
    assert_eq!(r.body["error"]["code"], "not_owner");
    let r = call(
        &k,
        Method::POST,
        "/api/builder/poll",
        Some(&token),
        None,
        poll.clone(),
    )
    .await;
    assert_eq!(r.body["error"]["code"], "not_owner");
    let r = k
        .raw()
        .post(format!("{}/api/builder/poll", k.base))
        .header("X-Forwarded-For", MAC_IP)
        .bearer_auth(&token)
        .json(&poll)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN, "X-Kiwi is required");

    // A long poll: Kiwi Builds is off, so there is no work, not even a health check.
    let started = std::time::Instant::now();
    let r = call(
        &k,
        Method::POST,
        "/api/builder/poll",
        Some(&token),
        Some(MAC_IP),
        poll,
    )
    .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.body);
    assert!(started.elapsed() >= Duration::from_millis(250));
    // The builder is a program, not a device.
    assert_eq!(k.kiwi.db.devices().unwrap().len(), devices);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ios_apps_build_on_the_mac_builder_and_are_signed_by_kiwi() {
    let k = kiwi().await;
    let phone = k.phone();
    // Without a builder: the Mac is missing.
    assert_eq!(
        phone.ok("/api/automation").await["checks"],
        json!([{"name": "macOS", "ready": false}])
    );
    let r = add(&k, "youtube").await;
    assert_eq!(r.body["error"]["code"], "needs_mac");
    assert!(
        r.body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Settings › Mac builder"),
        "{}",
        r.body
    );
    assert_eq!(phone.ok("/api/me").await["host"]["builds_ios"], false);

    let (mac, _) = Mac::pair(&k).await;
    // It checks the Mac's setup when it connects.
    for _ in 0..500 {
        let checks = phone.ok("/api/automation").await["checks"].clone();
        if checks.to_string().contains("App Store sign-in") {
            assert_eq!(
                checks,
                json!([{"name": "Mac builder", "ready": true},
                    {"name": "Mac build tools", "ready": true},
                    {"name": "App Store sign-in", "ready": false}])
            );
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    // And again when asked.
    std::fs::write(mac.automation().join("signed-in"), "").unwrap();
    let r = phone.post("/api/builder/check", json!({})).await;
    assert_eq!(r.status, StatusCode::ACCEPTED, "{}", r.body);
    for _ in 0..500 {
        let checks = phone.ok("/api/builder").await["builder"]["checks"].clone();
        if checks[2]["ready"] == true {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        phone.ok("/api/builder").await["builder"]["checks"][2]["ready"],
        true
    );

    let r = add(&k, "youtube").await;
    assert_eq!(r.status, StatusCode::ACCEPTED, "{}", r.body);
    let done = finished(&k, "youtube").await;
    assert_eq!(done.status, "ready", "{done:?}");
    assert_eq!(done.stage, "Signed and available in Kiwi");
    assert_eq!(k.signer.signed.load(Ordering::SeqCst), 1);
    let app = phone
        .ok("/api/apps/kiwi-youtube/com.google.ios.youtube")
        .await;
    assert!(app["state"]["install_url"].is_string(), "{app}");

    // The IPA and its provenance are on Kiwi now, and no longer on the Mac.
    let release = &app["releases"][0];
    let sha = k
        .kiwi
        .catalog
        .source("kiwi-youtube")
        .unwrap()
        .source
        .catalog
        .apps[0]
        .releases[0]
        .sha256
        .clone()
        .unwrap();
    let artifacts = k.kiwi.config.data_dir.join("automation/artifacts");
    assert!(artifacts.join(format!("{sha}.ipa")).is_file(), "{release}");
    let provenance: Value =
        serde_json::from_slice(&std::fs::read(artifacts.join(format!("{sha}.json"))).unwrap())
            .unwrap();
    assert_eq!(provenance["sha256"], sha);
    for _ in 0..200 {
        if std::fs::read_dir(mac.automation().join("artifacts"))
            .map(|d| d.count() == 0)
            .unwrap_or(true)
        {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert_eq!(
        std::fs::read_dir(mac.automation().join("artifacts"))
            .unwrap()
            .count(),
        0,
        "IPAs don't pile up on the Mac"
    );
    assert_eq!(
        phone.ok("/api/builder").await["builder"]["task"],
        Value::Null
    );
    mac.stop().await;
}

/// A minimal patched YouTube, as the fake runner makes it.
fn youtube_ipa(revision: u64) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let options = zip::write::SimpleFileOptions::default();
    zip.start_file("Payload/YouTube.app/Info.plist", options)
        .unwrap();
    write!(
        zip,
        r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>com.google.ios.youtube</string>
<key>CFBundleShortVersionString</key><string>21.44.1</string>
<key>CFBundleVersion</key><string>21.44.1</string>
<key>KiwiBuildRevision</key><integer>{revision}</integer>
<key>MinimumOSVersion</key><string>17.0</string>
<key>CFBundleSupportedPlatforms</key><array><string>iPhoneOS</string></array>
<key>UIDeviceFamily</key><array><integer>1</integer></array>
</dict></plist>"#
    )
    .unwrap();
    zip.start_file("Payload/YouTube.app/YouTube", options)
        .unwrap();
    zip.write_all(&vec![0x78; 300_000]).unwrap();
    zip.finish().unwrap().into_inner()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uploads_are_checked_against_their_digest_and_the_order_of_calls() {
    let k = kiwi().await;
    let token = pair_raw(&k).await;
    assert_eq!(add(&k, "youtube").await.status, StatusCode::ACCEPTED);
    let job_id = k.kiwi.automation.snapshot().jobs[0].id.clone();

    let plan = task(&k, &token, "plan").await;
    assert_eq!(plan["id"], job_id.as_str());
    assert_eq!(plan["recipe"], "youtube");
    assert_eq!(plan["request"]["app"], "youtube");
    assert!(plan["deadline"].is_string());
    let r = events(
        &k,
        &token,
        &job_id,
        json!([{"event": "stage", "stage": "Checking the App Store"}]),
    )
    .await;
    assert_eq!(r.body, json!({"cancel": false}));
    assert_eq!(
        k.kiwi.automation.snapshot().jobs[0].stage,
        "Checking the App Store"
    );
    let r = events(&k, &token, &job_id, json!([])).await;
    assert_eq!(r.body, json!({"cancel": false}), "a heartbeat");
    let r = events(
        &k,
        &token,
        &job_id,
        json!([{"event": "plan", "plan": {"key": "a".repeat(64),
        "version": "21.44.1", "external_id": "1234", "tweaks": [], "name": null}}]),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let r = finish(&k, &token, &job_id, json!({"status": "ok"})).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.body);

    let build = task(&k, &token, "build").await;
    assert_eq!(build["id"], job_id.as_str());
    let sequence = build["request"]["sequence"].as_u64().unwrap();
    let ipa = youtube_ipa(sequence);
    let sha = hex::encode(Sha256::digest(&ipa));
    // The artifact event comes first.
    let r = upload(&k, &token, &job_id, &sha, ipa.clone()).await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.body);
    let r = events(
        &k,
        &token,
        &job_id,
        json!([{"event": "stage", "stage": "Injecting tweaks"},
        {"event": "artifact", "sha256": sha, "version": "21.44.1", "build": "21.44.1",
         "revision": sequence, "min_os": "17.0", "size": ipa.len(), "notes": "Fixture"}]),
    )
    .await;
    assert_eq!(r.body, json!({"cancel": false}));
    // Bytes that don't match the digest are refused and not kept.
    let mut damaged = ipa.clone();
    damaged[100] ^= 1;
    let r = upload(&k, &token, &job_id, &sha, damaged).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.body);
    let automation = k.kiwi.config.data_dir.join("automation");
    assert!(!automation.join(format!("artifacts/{sha}.ipa")).exists());
    assert!(!automation.join(format!("{job_id}.upload")).exists());
    let r = upload(&k, &token, &job_id, &"b".repeat(64), ipa.clone()).await;
    assert_eq!(
        r.status,
        StatusCode::CONFLICT,
        "another digest than the event's"
    );
    // A build isn't done before its upload.
    let r = finish(&k, &token, &job_id, json!({"status": "ok"})).await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.body);
    let r = upload(&k, &token, &job_id, &sha, ipa.clone()).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.body);
    let r = finish(
        &k,
        &token,
        &job_id,
        json!({"status": "ok", "provenance": {"sha256": "c".repeat(64)}}),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.body);
    let r = finish(
        &k,
        &token,
        &job_id,
        json!({"status": "ok", "provenance": {"sha256": sha, "sequence": sequence}}),
    )
    .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.body);

    let done = finished(&k, "youtube").await;
    assert_eq!(done.status, "ready", "{done:?}");
    assert!(automation.join(format!("artifacts/{sha}.json")).is_file());
    // The task is over.
    let r = events(&k, &token, &job_id, json!([])).await;
    assert_eq!(r.status, StatusCode::GONE);
    assert_eq!(r.body["error"]["code"], "task_gone");
    let r = finish(&k, &token, &job_id, json!({"status": "ok"})).await;
    assert_eq!(r.status, StatusCode::GONE);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn runner_errors_and_invalid_events_fail_the_job_as_locally() {
    let k = kiwi().await;
    let token = pair_raw(&k).await;
    add(&k, "youtube").await;
    let plan = task(&k, &token, "plan").await;
    let id = plan["id"].as_str().unwrap();
    let r = events(
        &k,
        &token,
        id,
        json!([{"event": "error",
        "message": "Setup required on the Mac: App Store sign-in", "blocked": true}]),
    )
    .await;
    assert_eq!(r.body, json!({"cancel": false}));
    finish(&k, &token, id, json!({"status": "failed"})).await;
    let done = finished(&k, "youtube").await;
    assert_eq!(done.status, "blocked", "{done:?}");
    assert_eq!(
        done.error.as_deref(),
        Some("Setup required on the Mac: App Store sign-in")
    );

    // An event Kiwi doesn't know stops the build, as from a local runner.
    k.phone()
        .post("/api/automation/youtube/check", json!({}))
        .await;
    let plan = task(&k, &token, "plan").await;
    let id = plan["id"].as_str().unwrap();
    let r = events(
        &k,
        &token,
        id,
        json!([{"event": "shell", "command": "true"}]),
    )
    .await;
    assert_eq!(r.body, json!({"cancel": true}));
    let done = finished(&k, "youtube").await;
    assert_eq!(done.error.as_deref(), Some("Unknown builder event"));
    let r = events(&k, &token, id, json!([])).await;
    assert_eq!(r.body, json!({"cancel": true}));
    let r = finish(&k, &token, id, json!({"status": "cancelled"})).await;
    assert_eq!(r.status, StatusCode::GONE);
    // A stage over the limit too.
    k.phone()
        .post("/api/automation/youtube/check", json!({}))
        .await;
    let plan = task(&k, &token, "plan").await;
    let id = plan["id"].as_str().unwrap();
    let r = events(
        &k,
        &token,
        id,
        json!([{"event": "stage", "stage": "x".repeat(257)}]),
    )
    .await;
    assert_eq!(r.body, json!({"cancel": true}));
    let done = finished(&k, "youtube").await;
    assert_eq!(done.error.as_deref(), Some("Build stage exceeds its limit"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_stops_the_runner_on_the_mac() {
    let k = kiwi().await;
    let (mac, saved) = Mac::pair(&k).await;
    let pause = mac.automation().join("pause-plan");
    std::fs::write(&pause, "").unwrap();
    assert_eq!(add(&k, "youtube").await.status, StatusCode::ACCEPTED);
    let phone = k.phone();
    for _ in 0..500 {
        let task = phone.ok("/api/builder").await["builder"]["task"].clone();
        if task["stage"] == "Checking the App Store" {
            assert_eq!(task["app"], "youtube");
            assert_eq!(task["name"], "YouTube");
            assert_eq!(task["command"], "plan");
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let r = phone
        .post("/api/automation/youtube/cancel", json!({}))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(finished(&k, "youtube").await.status, "cancelled");
    let terminated = mac.automation().join("terminated");
    wait_until(|| terminated.exists()).await;

    // The builder carries on with the next task.
    std::fs::remove_file(&pause).unwrap();
    phone.post("/api/automation/youtube/check", json!({})).await;
    assert_eq!(finished(&k, "youtube").await.status, "ready");
    mac.stop().await;
    // Stopping the builder program stops its runner too, and Kiwi hears why.
    let mac = Mac::start(&k, &k.dir.path().join("Kiwi Builder"), saved, |_| {});
    std::fs::write(mac.automation().join("pause-plan"), "").unwrap();
    std::fs::remove_file(&terminated).unwrap();
    let check = phone
        .post("/api/automation/youtube/check", json!({"force": true}))
        .await;
    assert_eq!(check.status, StatusCode::ACCEPTED, "{}", check.body);
    job(&k, "youtube", |j| j.stage == "Checking the App Store").await;
    mac.stop().await;
    let done = finished(&k, "youtube").await;
    assert_eq!(done.status, "failed");
    assert_eq!(done.error.as_deref(), Some("Test Mac stopped the build."));
    assert_eq!(done.error_kind.as_deref(), Some("retry"));
    assert!(terminated.exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lost_builder_fails_the_job_as_retryable() {
    let k = kiwi().await;
    let token = pair_raw(&k).await;
    add(&k, "youtube").await;
    let plan = task(&k, &token, "plan").await;
    let id = plan["id"].as_str().unwrap();
    let r = events(
        &k,
        &token,
        id,
        json!([{"event": "plan", "plan": {"key": "a".repeat(64),
        "version": "21.44.1", "external_id": "1234", "tweaks": [], "name": null}}]),
    )
    .await;
    assert_eq!(r.status, StatusCode::OK);
    finish(&k, &token, id, json!({"status": "ok"})).await;
    task(&k, &token, "build").await;
    // The Mac goes quiet.
    let done = finished(&k, "youtube").await;
    assert_eq!(done.status, "failed");
    assert_eq!(
        done.error.as_deref(),
        Some("Lost contact with Test Mac during the build.")
    );
    assert_eq!(done.error_kind.as_deref(), Some("retry"));
    let app = &k.kiwi.automation.snapshot().apps["youtube"];
    assert_eq!(app.failed_attempts, 0, "not the recipe's fault");
    // Its late calls find nothing.
    assert_eq!(
        events(&k, &token, id, json!([])).await.status,
        StatusCode::GONE
    );
    assert_eq!(
        finish(&k, &token, id, json!({"status": "ok"})).await.status,
        StatusCode::GONE
    );
    let r = upload(&k, &token, id, &"a".repeat(64), vec![1, 2, 3]).await;
    assert_eq!(r.status, StatusCode::GONE);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tasks_are_gone_after_a_restart_and_unpairing_revokes_the_builder() {
    let k = kiwi().await;
    let token = pair_raw(&k).await;
    add(&k, "youtube").await;
    let plan = task(&k, &token, "plan").await;
    let id = plan["id"].as_str().unwrap().to_owned();

    // Kiwi restarts: the builder stays paired, the task is gone.
    let dir = k.stop().await;
    let k = TestKiwi::start_on_mac(dir, FakeDevices::with_phone()).await;
    not_a_mac(&k);
    fake_ios_runner(&k);
    assert_eq!(
        events(&k, &token, &id, json!([])).await.status,
        StatusCode::GONE
    );
    let r = call(
        &k,
        Method::POST,
        "/api/builder/poll",
        Some(&token),
        None,
        hello(),
    )
    .await;
    assert!(r.status.is_success(), "{}", r.body);
    assert_eq!(k.phone().ok("/api/builder").await["builder"]["name"], NAME);
    // Stopping Kiwi cancelled the job.
    let done = finished(&k, "youtube").await;
    assert!(
        ["Cancelled", "Interrupted by restart"].contains(&done.stage.as_str()),
        "{done:?}"
    );

    // Removing it ends its task and cancels the iOS jobs waiting for it.
    k.phone()
        .post("/api/automation/youtube/check", json!({}))
        .await;
    let plan = task(&k, &token, "plan").await;
    let id = plan["id"].as_str().unwrap().to_owned();
    assert_eq!(add(&k, "twitch").await.status, StatusCode::ACCEPTED);
    let r = k.phone().send(Method::DELETE, "/api/builder", None).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.body);
    let done = finished(&k, "youtube").await;
    assert_eq!(done.status, "failed");
    assert_eq!(
        done.error.as_deref(),
        Some("Test Mac was removed from Kiwi during the build.")
    );
    let twitch = finished(&k, "twitch").await;
    assert_eq!(twitch.status, "cancelled");
    assert!(twitch.error.unwrap().contains("Settings › Mac builder"));
    let r = events(&k, &token, &id, json!([])).await;
    assert_eq!(r.status, StatusCode::UNAUTHORIZED);
    assert_eq!(r.body["error"]["code"], "builder_unpaired");
    let r = call(
        &k,
        Method::POST,
        "/api/builder/poll",
        Some(&token),
        None,
        hello(),
    )
    .await;
    assert_eq!(r.body["error"]["code"], "builder_unpaired");
    let phone = k.phone();
    assert_eq!(phone.ok("/api/builder").await["builder"], Value::Null);
    assert_eq!(
        phone.ok("/api/automation").await["checks"],
        json!([{"name": "macOS", "ready": false}])
    );
    assert_eq!(
        phone
            .post("/api/automation/youtube/check", json!({}))
            .await
            .body["error"]["code"],
        "needs_mac"
    );
    assert_eq!(
        phone
            .send(Method::DELETE, "/api/builder", None)
            .await
            .status,
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        phone.post("/api/builder/check", json!({})).await.status,
        StatusCode::CONFLICT
    );

    // Pairing another Mac replaces the first: its token stops working.
    let first = pair_raw(&k).await;
    let second = pair_raw(&k).await;
    let r = call(
        &k,
        Method::POST,
        "/api/builder/poll",
        Some(&first),
        None,
        hello(),
    )
    .await;
    assert_eq!(r.body["error"]["code"], "builder_unpaired");
    let r = call(
        &k,
        Method::POST,
        "/api/builder/poll",
        Some(&second),
        None,
        hello(),
    )
    .await;
    assert!(r.status.is_success(), "{}", r.body);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ios_jobs_wait_for_an_offline_or_outdated_builder_while_android_jobs_build() {
    let k = kiwi().await;
    fake_android_runner(&k);
    let token = pair_raw(&k).await;
    let saved = Saved {
        server: k.base.clone(),
        token,
        name: NAME.into(),
        kiwi: None,
    };
    let phone = k.phone();
    // Nothing heard from the Mac for a while: offline.
    tokio::time::sleep(Duration::from_millis(1600)).await;
    let builder = phone.ok("/api/builder").await["builder"].clone();
    assert_eq!(builder["online"], false);
    assert_eq!(
        builder["checks"][0],
        json!({"name": "Mac builder", "ready": false})
    );

    assert_eq!(add(&k, "youtube").await.status, StatusCode::ACCEPTED);
    let waiting = job(&k, "youtube", |j| j.stage == "Waiting for Test Mac").await;
    assert_eq!(waiting.status, "queued");
    assert_eq!(
        add(&k, "android-youtube").await.status,
        StatusCode::ACCEPTED
    );
    let android = finished(&k, "android-youtube").await;
    assert_eq!(android.status, "ready", "{android:?}");
    assert_eq!(
        k.kiwi
            .automation
            .snapshot()
            .jobs
            .iter()
            .find(|j| j.app == "youtube")
            .unwrap()
            .status,
        "queued"
    );

    // A builder with another runner: online, but the job waits for its update.
    let data = k.dir.path().join("Kiwi Builder");
    let outdated = Mac::start(&k, &data, saved.clone(), |b| {
        b.hello.runner_sha256 = "0".repeat(64);
    });
    job(&k, "youtube", |j| {
        j.stage == "Waiting for an update of Kiwi on Test Mac"
    })
    .await;
    let builder = phone.ok("/api/builder").await["builder"].clone();
    assert_eq!(builder["online"], true);
    assert_eq!(builder["compatible"], false);
    assert_eq!(builder["checks"][0]["ready"], false);
    outdated.stop().await;

    // Updated, it builds.
    let mac = Mac::start(&k, &data, saved, |_| {});
    let done = finished(&k, "youtube").await;
    assert_eq!(done.status, "ready", "{done:?}");
    mac.stop().await;
}
