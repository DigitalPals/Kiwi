mod common;

use common::*;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};
use std::sync::atomic::Ordering;

async fn finished(k: &TestKiwi) -> Value {
    finished_app(k, "youtube").await
}

async fn finished_app(k: &TestKiwi, app: &str) -> Value {
    let phone = k.phone();
    for _ in 0..300 {
        let s = phone.ok(&format!("/api/automation/{app}")).await;
        if s["jobs"][0]["finished_at"].is_string() {
            return s;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("automation did not finish");
}

fn fake_runner(k: &TestKiwi, pause: bool) {
    let script = k.dir.path().join("fake-builder.py");
    let text = format!(
        r#"
import sys,json,pathlib,zipfile,plistlib,hashlib,time
command=sys.argv[1]
recipe=sys.argv[sys.argv.index('--recipe')+1]
bundle=dict(youtube='com.google.ios.youtube',twitch='tv.twitch',x='com.atebits.Tweetie2',reddit='com.reddit.Reddit')[recipe]
version='2026.43.0' if recipe=='reddit' else '21.44.1'
root=pathlib.Path(sys.argv[sys.argv.index('--data-dir')+1])/'automation'
request=json.loads(pathlib.Path(sys.argv[sys.argv.index('--request')+1]).read_text())
def emit(kind,**data):print(json.dumps(dict(event=kind,**data)),flush=True)
if command=='plan':
 emit('health',checks=[dict(name='Fixture',ready=True)],ready=True)
 {pause}
 key='a'*64
 if sorted(request['tweaks'])!=sorted(['youpip','ryd','ytabconfig','youquality']):
  key=hashlib.sha256(json.dumps(sorted(request['tweaks'])).encode()).hexdigest()
 if recipe!='youtube':
  key=hashlib.sha256(json.dumps([recipe,request['tweaks'],request.get('name')]).encode()).hexdigest()
 emit('plan',plan=dict(key=key,version=version,external_id='1234',tweaks=request['tweaks'],name=request.get('name')))
else:
 number=version
 p=root/'candidate.ipa'
 info=dict(CFBundleIdentifier=bundle,CFBundleShortVersionString=version,CFBundleVersion=number,KiwiBuildRevision=request['sequence'],MinimumOSVersion='17.0',CFBundleSupportedPlatforms=['iPhoneOS'],UIDeviceFamily=[1])
 with zipfile.ZipFile(p,'w') as z:
  z.writestr('Payload/YouTube.app/Info.plist',plistlib.dumps(info))
  z.writestr('Payload/YouTube.app/YouTube',b'x'*300000)
 sha=hashlib.sha256(p.read_bytes()).hexdigest()
 dest=root/'artifacts'
 dest.mkdir(exist_ok=True)
 p.rename(dest/(sha+'.ipa'))
 emit('artifact',sha256=sha,version=version,build=number,revision=request['sequence'],min_os='17.0',size=(dest/(sha+'.ipa')).stat().st_size,notes='Fixture')
"#,
        pause = if pause { "time.sleep(30)" } else { "pass" }
    );
    std::fs::write(&script, text).unwrap();
    *k.kiwi.automation.runner.write().unwrap() = script;
    // The fake builds iOS apps on any system.
    k.kiwi.automation.builds_ios.store(true, Ordering::SeqCst);
}

#[tokio::test]
async fn ios_apps_need_a_mac_and_android_apps_build_anywhere() {
    const PACKAGE: &str = "app.morphe.android.youtube";
    let k = TestKiwi::start().await;
    fake_android_runner(&k, PACKAGE);
    // As on Linux: the iOS runner needs macOS.
    k.kiwi.automation.builds_ios.store(false, Ordering::SeqCst);
    let phone = k.phone();
    phone
        .send(
            Method::PATCH,
            "/api/settings",
            Some(json!({"kiwi_builds": true})),
        )
        .await;

    // The iOS setup checks name the missing Mac, and iOS apps can't be added or checked.
    let overview = phone.ok("/api/automation").await;
    assert_eq!(
        overview["checks"],
        json!([{"name": "macOS", "ready": false}])
    );
    let youtube = phone.ok("/api/automation/youtube").await;
    assert_eq!(youtube["checks"], overview["checks"]);
    let added = phone
        .post(
            "/api/automation/youtube/add",
            youtube["preferences"].clone(),
        )
        .await;
    assert_eq!(added.status, StatusCode::CONFLICT, "{}", added.body);
    assert_eq!(added.body["error"]["code"], "needs_mac");
    assert!(
        added.body["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("iOS apps can only be built on a Mac."),
        "{}",
        added.body
    );
    assert_eq!(phone.ok("/api/automation/youtube").await["added"], false);

    let check = phone.post("/api/automation/youtube/check", json!({})).await;
    assert_eq!(check.body["error"]["code"], "needs_mac", "{}", check.body);

    // Android apps build as usual.
    let android = phone.ok("/api/automation/android-youtube").await;
    let added = phone
        .post(
            "/api/automation/android-youtube/add",
            android["preferences"].clone(),
        )
        .await;
    assert_eq!(added.status, StatusCode::ACCEPTED, "{}", added.body);
    let ready = finished_app(&k, "android-youtube").await;
    assert_eq!(ready["jobs"][0]["status"], "ready", "{ready}");
    assert!(
        k.kiwi
            .automation
            .snapshot()
            .jobs
            .iter()
            .all(|j| j.app == "android-youtube")
    );

    // An iOS app added on a Mac whose data moved here waits: no schedule, no checks.
    fake_runner(&k, true);
    let added = phone
        .post(
            "/api/automation/youtube/add",
            youtube["preferences"].clone(),
        )
        .await;
    assert_eq!(added.status, StatusCode::ACCEPTED, "{}", added.body);
    phone
        .post("/api/automation/youtube/cancel", json!({}))
        .await;
    assert_eq!(finished(&k).await["jobs"][0]["status"], "cancelled");
    k.kiwi.automation.builds_ios.store(false, Ordering::SeqCst);
    let waiting = phone.ok("/api/automation/youtube").await;
    assert_eq!(waiting["added"], true);
    assert_eq!(waiting["next_check_at"], Value::Null, "{waiting}");
    assert_eq!(waiting["checks"], overview["checks"]);
    let check = phone.post("/api/automation/youtube/check", json!({})).await;
    assert_eq!(check.body["error"]["code"], "needs_mac", "{}", check.body);
}

#[tokio::test]
async fn kiwi_builds_is_off_until_turned_on_and_turning_it_off_stops_builds() {
    let k = TestKiwi::start().await;
    fake_runner(&k, true);
    let phone = k.phone();
    assert_eq!(phone.ok("/api/settings").await["kiwi_builds"], false);
    for (method, path) in [
        (Method::GET, "/api/automation"),
        (Method::GET, "/api/automation/youtube"),
        (Method::POST, "/api/automation/youtube/check"),
    ] {
        let r = phone.send(method, path, Some(json!({}))).await;
        assert_eq!(r.status, StatusCode::CONFLICT, "{path}");
        assert_eq!(r.body["error"]["code"], "builds_off", "{path}");
    }

    // Turned on, nothing is added until the owner picks an app.
    let on = phone
        .send(
            Method::PATCH,
            "/api/settings",
            Some(json!({"kiwi_builds": true})),
        )
        .await;
    assert_eq!(on.body["kiwi_builds"], true);
    let overview = phone.ok("/api/automation").await;
    let apps = overview["apps"].as_array().unwrap();
    assert!(apps.iter().all(|a| a["added"] == false), "{overview}");
    let initial = phone.ok("/api/automation/youtube").await;
    let added = phone
        .post(
            "/api/automation/youtube/add",
            initial["preferences"].clone(),
        )
        .await;
    assert_eq!(added.status, StatusCode::ACCEPTED, "{}", added.body);

    // Turning it off cancels the build.
    phone
        .send(
            Method::PATCH,
            "/api/settings",
            Some(json!({"kiwi_builds": false})),
        )
        .await;
    let mut status = None;
    for _ in 0..300 {
        let state = k.kiwi.automation.snapshot();
        if let Some(job) = state.jobs.first().filter(|j| j.finished_at.is_some()) {
            status = Some(job.status.clone());
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(status.as_deref(), Some("cancelled"));
    let r = phone.get("/api/automation").await;
    assert_eq!(r.body["error"]["code"], "builds_off");
}

#[tokio::test]
async fn builds_are_signed_before_publication_deduplicated_and_keep_the_last_release() {
    let k = TestKiwi::start_building().await;
    fake_runner(&k, false);
    k.signer.hold.store(true, Ordering::SeqCst);
    let phone = k.phone();
    assert_eq!(
        phone
            .post("/api/automation/youtube/check", json!({}))
            .await
            .status,
        StatusCode::ACCEPTED
    );
    for _ in 0..150 {
        if !k.kiwi.db.builds().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(
        k.kiwi.catalog.source("kiwi-youtube").is_none(),
        "unsigned candidate must not be published"
    );
    k.signer.hold.store(false, Ordering::SeqCst);
    let ready = finished(&k).await;
    assert_eq!(ready["jobs"][0]["status"], "ready", "{ready}");
    let app = phone
        .ok("/api/apps/kiwi-youtube/com.google.ios.youtube")
        .await;
    assert!(app["state"]["install_url"].is_string());
    assert_eq!(k.signer.signed.load(Ordering::SeqCst), 1);

    phone.post("/api/automation/youtube/check", json!({})).await;
    assert_eq!(finished(&k).await["jobs"][0]["status"], "unchanged");
    assert_eq!(k.signer.signed.load(Ordering::SeqCst), 1);
    assert_eq!(
        phone
            .post("/api/sources/kiwi-youtube/refresh", json!({}))
            .await
            .status,
        StatusCode::OK
    );

    let runner = k.kiwi.automation.runner.read().unwrap().clone();
    let next_recipe = std::fs::read_to_string(&runner)
        .unwrap()
        .replace("key='a'*64", "key='b'*64");
    std::fs::write(runner, next_recipe).unwrap();
    *k.signer.fail.lock().unwrap() = Some("Fixture signing failure".into());
    phone
        .post("/api/automation/youtube/check", json!({"force":true}))
        .await;
    assert_eq!(finished(&k).await["jobs"][0]["status"], "failed");
    let still_ready = phone
        .ok("/api/apps/kiwi-youtube/com.google.ios.youtube")
        .await;
    assert_eq!(
        still_ready["releases"][0]["build"],
        app["releases"][0]["build"]
    );
    assert!(still_ready["state"]["install_url"].is_string());
    for _ in 0..2 {
        phone.post("/api/automation/youtube/check", json!({})).await;
        assert_eq!(finished(&k).await["jobs"][0]["status"], "failed");
    }
    let before = k.kiwi.db.builds().unwrap().len();
    phone.post("/api/automation/youtube/check", json!({})).await;
    let blocked = finished(&k).await;
    assert_eq!(blocked["jobs"][0]["status"], "blocked");
    assert_eq!(blocked["failed_attempts"], 3);
    assert_eq!(k.kiwi.db.builds().unwrap().len(), before);

    // Cache cleanup must preserve the published input even when it is old.
    let source = k.kiwi.catalog.source("kiwi-youtube").unwrap();
    let sha = source.source.catalog.apps[0].releases[0]
        .sha256
        .as_ref()
        .unwrap();
    let artifacts = k.kiwi.config.data_dir.join("automation/artifacts");
    let retained = artifacts.join(format!("{sha}.ipa"));
    let orphan = artifacts.join(format!("{}.ipa", "f".repeat(64)));
    std::fs::write(&orphan, b"unused candidate").unwrap();
    let old = std::time::SystemTime::now() - std::time::Duration::from_secs(8 * 86400);
    for path in [&retained, &orphan] {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(std::fs::FileTimes::new().set_modified(old))
            .unwrap();
    }
    kiwi_server::automation::cleanup(&k.kiwi).await.unwrap();
    assert!(retained.is_file());
    assert!(!orphan.exists());
}

#[tokio::test]
async fn patch_revisions_offer_updates_and_select_the_requested_artifact() {
    let k = TestKiwi::start_building().await;
    fake_runner(&k, false);
    let phone = k.phone();
    let path = "/api/apps/kiwi-youtube/com.google.ios.youtube";
    phone.post("/api/automation/youtube/check", json!({})).await;
    assert_eq!(finished(&k).await["jobs"][0]["status"], "ready");
    let first = phone.ok(path).await;
    assert_eq!(first["releases"][0]["build"], "21.44.1");
    assert_eq!(first["releases"][0]["revision"], 1);
    let first_id = first["state"]["build"]["id"].clone();
    let ota = ota_path(first["state"]["install_url"].as_str().unwrap());
    let response = k
        .raw()
        .get(format!("{}{ota}/app.ipa", k.base))
        .header("User-Agent", INSTALLER_UA)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.bytes().await.unwrap();
    let mut installed = false;
    for _ in 0..100 {
        let app = phone.ok(path).await;
        if app["state"]["action"] == "open" {
            assert_eq!(app["state"]["installed"]["revision"], 1);
            installed = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert!(installed, "OTA install must retain the patch revision");

    phone
        .post("/api/automation/youtube/check", json!({"force":true}))
        .await;
    assert_eq!(finished(&k).await["jobs"][0]["status"], "ready");
    let second = phone.ok(path).await;
    assert_eq!(second["state"]["action"], "update");
    assert_eq!(second["state"]["update_reason"], "new_version");
    assert_eq!(second["state"]["installed"]["revision"], 1);
    assert_eq!(second["state"]["target"]["revision"], 2);
    assert_eq!(second["releases"][0]["build"], "21.44.1");
    assert_eq!(second["releases"][1]["build"], "21.44.1");
    let second_id = second["state"]["build"]["id"].clone();
    assert_ne!(first_id, second_id);

    for (revision, id) in [(1, first_id), (2, second_id)] {
        let selected = phone
            .post(
                &format!("{path}/prepare"),
                json!({"version":"21.44.1","build":"21.44.1","revision":revision}),
            )
            .await;
        assert_eq!(selected.status, StatusCode::ACCEPTED, "{}", selected.body);
        assert_eq!(selected.body["id"], id);
        assert_eq!(selected.body["revision"], revision);
    }
    let missing = phone
        .post(
            &format!("{path}/prepare"),
            json!({"version":"21.44.1","revision":999}),
        )
        .await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
}

#[tokio::test]
async fn configuration_tracks_the_published_build_and_running_jobs_keep_their_choices() {
    let k = TestKiwi::start_building().await;
    fake_runner(&k, false);
    let phone = k.phone();
    let initial = phone.ok("/api/automation/youtube").await;
    assert!(initial["published"].is_null());
    assert!(initial["source_enabled"].is_null());
    assert!(initial["next_check_at"].is_string());
    assert_eq!(initial["available_tweaks"].as_array().unwrap().len(), 4);
    let saved = phone
        .send(
            Method::PATCH,
            "/api/automation/youtube",
            Some(json!({
                "enabled":false,"interval_minutes":150,"tweaks":[]
            })),
        )
        .await;
    assert_eq!(saved.status, StatusCode::OK);
    assert!(saved.body["next_check_at"].is_null());
    assert!(
        saved.body["jobs"].as_array().unwrap().is_empty(),
        "saving must not force a build"
    );
    let persisted: Value =
        serde_json::from_str(&k.kiwi.db.meta("automation_v2").unwrap().unwrap()).unwrap();
    assert_eq!(
        persisted["apps"]["youtube"]["preferences"],
        saved.body["preferences"]
    );
    phone.post("/api/automation/youtube/check", json!({})).await;
    let first = finished(&k).await;
    assert_eq!(first["jobs"][0]["status"], "ready", "{first}");
    assert_eq!(first["published"]["revision"], 1);
    assert_eq!(first["published"]["tweaks"], json!([]));
    assert_eq!(first["configuration_changed"], false);
    assert!(
        first["next_check_at"].is_null(),
        "manual builds work with the schedule off"
    );
    let sources = phone.ok("/api/sources").await;
    let built_in = sources
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["id"] == "kiwi-youtube")
        .unwrap();
    assert_eq!(built_in["kind"], "local_build");
    assert_eq!(
        phone
            .send(Method::DELETE, "/api/sources/kiwi-youtube", None)
            .await
            .status,
        StatusCode::BAD_REQUEST
    );

    let change = phone
        .send(
            Method::PATCH,
            "/api/automation/youtube",
            Some(json!({
                "enabled":false,"interval_minutes":60,"tweaks":["youpip"]
            })),
        )
        .await;
    assert_eq!(change.body["configuration_changed"], true);
    assert_eq!(change.body["published"]["tweaks"], json!([]));
    k.signer.hold.store(true, Ordering::SeqCst);
    phone.post("/api/automation/youtube/check", json!({})).await;
    wait_until(|| k.kiwi.db.builds().unwrap().len() == 2).await;
    phone
        .send(
            Method::PATCH,
            "/api/automation/youtube",
            Some(json!({
                "enabled":false,"interval_minutes":60,"tweaks":["ryd"]
            })),
        )
        .await;
    phone
        .send(
            Method::PATCH,
            "/api/sources/kiwi-youtube",
            Some(json!({"enabled":false})),
        )
        .await;
    k.signer.hold.store(false, Ordering::SeqCst);
    let second = finished(&k).await;
    assert_eq!(second["jobs"][0]["status"], "ready", "{second}");
    assert_eq!(
        second["jobs"][0]["preferences"]["tweaks"],
        json!(["youpip"])
    );
    assert_eq!(second["published"]["tweaks"], json!(["youpip"]));
    assert_eq!(second["preferences"]["tweaks"], json!(["ryd"]));
    assert_eq!(second["configuration_changed"], true);
    assert_eq!(
        second["source_enabled"], false,
        "publication must preserve visibility changes"
    );

    phone.post("/api/automation/youtube/check", json!({})).await;
    let third = finished(&k).await;
    assert_eq!(third["jobs"][0]["status"], "ready", "{third}");
    assert_eq!(third["published"]["tweaks"], json!(["ryd"]));
    assert_eq!(third["configuration_changed"], false);
    assert_eq!(third["source_enabled"], false);
}

#[tokio::test]
async fn jobs_are_single_flight_cancellable_and_preferences_are_validated() {
    let k = TestKiwi::start_building().await;
    fake_runner(&k, true);
    let phone = k.phone();
    let bad = phone
        .send(
            Method::PATCH,
            "/api/automation/youtube",
            Some(json!({"enabled":true,"interval_minutes":1,"tweaks":[]})),
        )
        .await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
    let good = phone
        .send(
            Method::PATCH,
            "/api/automation/youtube",
            Some(json!({"enabled":false,"interval_minutes":30,"tweaks":["youpip"]})),
        )
        .await;
    assert_eq!(good.body["preferences"]["enabled"], false);
    assert_eq!(
        phone
            .post("/api/automation/youtube/check", json!({}))
            .await
            .status,
        StatusCode::ACCEPTED
    );
    assert_eq!(
        phone
            .post("/api/automation/youtube/check", json!({}))
            .await
            .status,
        StatusCode::CONFLICT
    );
    phone
        .post("/api/automation/youtube/cancel", json!({}))
        .await;
    assert_eq!(finished(&k).await["jobs"][0]["status"], "cancelled");
    assert!(k.kiwi.catalog.source("kiwi-youtube").is_none());
}

#[tokio::test]
async fn apps_are_added_queued_behind_each_other_published_together_and_removed() {
    let k = TestKiwi::start_building().await;
    fake_runner(&k, false);
    let phone = k.phone();
    let overview = phone.ok("/api/automation").await;
    let ids: Vec<&str> = overview["apps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        ids,
        [
            "youtube",
            "youtube-music",
            "twitch",
            "x",
            "reddit",
            "android-youtube",
            "android-youtube-music",
            "android-reddit"
        ]
    );
    assert_eq!(overview["apps"][0]["platform"], "ios");
    assert_eq!(overview["apps"][5]["platform"], "android");
    assert_eq!(overview["apps"][0]["added"], true);
    assert_eq!(overview["apps"][2]["added"], false);
    assert_eq!(overview["apps"][3]["experimental"], true);
    assert_eq!(overview["apps"][3]["names"], json!(["Twitter", "X"]));
    assert!(overview["apps"][3]["warning"].is_string());
    assert!(
        !overview["apps"][2]["limitations"]
            .as_array()
            .unwrap()
            .is_empty()
    );

    let check = phone.post("/api/automation/twitch/check", json!({})).await;
    assert_eq!(check.status, StatusCode::BAD_REQUEST, "{}", check.body);
    let unknown = phone
        .post("/api/automation/instagram/check", json!({}))
        .await;
    assert_eq!(unknown.status, StatusCode::NOT_FOUND);
    let bad = phone
        .post(
            "/api/automation/twitch/add",
            json!({"enabled":true,"interval_minutes":360,"tweaks":["ryd"]}),
        )
        .await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);

    // YouTube holds the worker in signing; Twitch waits its turn.
    k.signer.hold.store(true, Ordering::SeqCst);
    phone.post("/api/automation/youtube/check", json!({})).await;
    wait_until(|| !k.kiwi.db.builds().unwrap().is_empty()).await;
    let added = phone
        .post(
            "/api/automation/twitch/add",
            json!({"enabled":false,"interval_minutes":360,"tweaks":[]}),
        )
        .await;
    assert_eq!(added.status, StatusCode::ACCEPTED, "{}", added.body);
    assert_eq!(added.body["added"], true);
    assert_eq!(added.body["jobs"][0]["status"], "queued");
    assert_eq!(added.body["jobs"][0]["stage"], "Waiting for YouTube");
    k.signer.hold.store(false, Ordering::SeqCst);
    assert_eq!(finished(&k).await["jobs"][0]["status"], "ready");
    let twitch = finished_app(&k, "twitch").await;
    assert_eq!(twitch["jobs"][0]["status"], "ready", "{twitch}");
    assert_eq!(twitch["published"]["revision"], 1);
    let built = phone.ok("/api/apps?source=kiwi-youtube").await;
    let names: Vec<&str> = built
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["name"].as_str().unwrap())
        .collect();
    assert_eq!(names.len(), 2, "{built}");
    assert!(names.contains(&"Twitch") && names.contains(&"YouTube"));
    assert!(phone.ok("/api/apps/kiwi-youtube/tv.twitch").await["state"]["install_url"].is_string());

    let removed = phone
        .send(Method::DELETE, "/api/automation/twitch", None)
        .await;
    assert_eq!(removed.status, StatusCode::OK);
    assert_eq!(removed.body["added"], false);
    assert!(removed.body["published"].is_null());
    assert_eq!(
        phone
            .ok("/api/apps?source=kiwi-youtube")
            .await
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(
        phone
            .ok("/api/apps/kiwi-youtube/com.google.ios.youtube")
            .await["state"]["install_url"]
            .is_string(),
        "removing one app keeps the others"
    );

    // Adding it again continues the revision numbering.
    phone
        .post(
            "/api/automation/twitch/add",
            json!({"enabled":false,"interval_minutes":360,"tweaks":[]}),
        )
        .await;
    let again = finished_app(&k, "twitch").await;
    assert_eq!(again["jobs"][0]["status"], "ready", "{again}");
    assert_eq!(again["published"]["revision"], 2);
}

#[tokio::test]
async fn reddit_is_supported_and_publishes_signed_updates_with_its_original_bundle_id() {
    let k = TestKiwi::start_building().await;
    fake_runner(&k, false);
    let phone = k.phone();
    let initial = phone.ok("/api/automation/reddit").await;
    assert_eq!(initial["experimental"], false);
    assert_eq!(initial["added"], false);
    assert_eq!(initial["app_version"], "2026.43.0");
    assert!(initial["available_tweaks"].as_array().unwrap().is_empty());

    // Ad removal and sideload fixes are required, not optional build selections.
    let bad = phone
        .post(
            "/api/automation/reddit/add",
            json!({"enabled":true,"interval_minutes":360,"tweaks":["redditfilter"]}),
        )
        .await;
    assert_eq!(bad.status, StatusCode::BAD_REQUEST);
    let added = phone
        .post("/api/automation/reddit/add", initial["preferences"].clone())
        .await;
    assert_eq!(added.status, StatusCode::ACCEPTED, "{}", added.body);
    let ready = finished_app(&k, "reddit").await;
    assert_eq!(ready["jobs"][0]["status"], "ready", "{ready}");
    assert_eq!(ready["published"]["version"], "2026.43.0");
    assert_eq!(ready["published"]["revision"], 1);
    assert_eq!(ready["published"]["tweaks"], json!([]));
    assert_eq!(ready["configuration_changed"], false);
    assert!(ready["next_check_at"].is_string());
    let path = "/api/apps/kiwi-youtube/com.reddit.Reddit";
    let app = phone.ok(path).await;
    assert_eq!(app["name"], "Reddit");
    assert_eq!(app["bundle_id"], "com.reddit.Reddit");
    assert_eq!(app["releases"][0]["version"], "2026.43.0");
    assert!(app["state"]["install_url"].is_string());
    assert_eq!(k.signer.signed.load(Ordering::SeqCst), 1);

    phone.post("/api/automation/reddit/check", json!({})).await;
    assert_eq!(
        finished_app(&k, "reddit").await["jobs"][0]["status"],
        "unchanged"
    );
    assert_eq!(k.signer.signed.load(Ordering::SeqCst), 1);
    phone
        .post("/api/automation/reddit/check", json!({"force":true}))
        .await;
    // The unchanged check reserved revision 2; revisions are never reused.
    assert_eq!(finished_app(&k, "reddit").await["published"]["revision"], 3);
    assert_eq!(
        phone.ok(path).await["releases"].as_array().unwrap().len(),
        2
    );
}

#[tokio::test]
async fn queued_jobs_cancel_without_running_and_names_are_part_of_the_build() {
    let k = TestKiwi::start_building().await;
    fake_runner(&k, false);
    let phone = k.phone();
    k.signer.hold.store(true, Ordering::SeqCst);
    phone.post("/api/automation/youtube/check", json!({})).await;
    wait_until(|| !k.kiwi.db.builds().unwrap().is_empty()).await;
    phone
        .post(
            "/api/automation/x/add",
            json!({"enabled":false,"interval_minutes":360,"tweaks":[],"name":"X"}),
        )
        .await;
    let cancelled = phone.post("/api/automation/x/cancel", json!({})).await;
    assert_eq!(cancelled.body["jobs"][0]["status"], "cancelled");
    assert!(cancelled.body["jobs"][0]["finished_at"].is_string());
    k.signer.hold.store(false, Ordering::SeqCst);
    assert_eq!(finished(&k).await["jobs"][0]["status"], "ready");
    assert_eq!(
        k.signer.signed.load(Ordering::SeqCst),
        1,
        "the cancelled job never ran"
    );

    phone.post("/api/automation/x/check", json!({})).await;
    let x = finished_app(&k, "x").await;
    assert_eq!(x["jobs"][0]["status"], "ready", "{x}");
    assert_eq!(x["published"]["name"], "X");
    assert_eq!(
        phone
            .ok("/api/apps/kiwi-youtube/com.atebits.Tweetie2")
            .await["name"],
        "X"
    );
    let renamed = phone
        .send(
            Method::PATCH,
            "/api/automation/x",
            Some(json!({"enabled":false,"interval_minutes":360,"tweaks":[],"name":"Twitter"})),
        )
        .await;
    assert_eq!(renamed.body["configuration_changed"], true);
    phone.post("/api/automation/x/check", json!({})).await;
    let rebuilt = finished_app(&k, "x").await;
    assert_eq!(rebuilt["jobs"][0]["status"], "ready", "{rebuilt}");
    assert_eq!(rebuilt["configuration_changed"], false);
    assert_eq!(
        phone
            .ok("/api/apps/kiwi-youtube/com.atebits.Tweetie2")
            .await["name"],
        "Twitter"
    );
}

#[tokio::test]
async fn upgrading_drops_apps_kiwi_no_longer_builds() {
    let k = TestKiwi::start_building().await;
    fake_runner(&k, false);
    let phone = k.phone();
    phone.post("/api/automation/youtube/check", json!({})).await;
    assert_eq!(finished(&k).await["jobs"][0]["status"], "ready");

    // A Kiwi that built Gboard Rambler before its recipe was removed.
    let mut row = (*k.kiwi.catalog.source("kiwi-youtube").unwrap()).clone();
    let mut gboard = serde_json::to_value(&row.source.catalog.apps[0]).unwrap();
    gboard["bundle_id"] = json!("com.google.keyboard");
    gboard["name"] = json!("Gboard Rambler");
    row.source
        .catalog
        .apps
        .push(serde_json::from_value(gboard).unwrap());
    k.kiwi.db.save_source(&row).unwrap();
    let mut state: Value =
        serde_json::from_str(&k.kiwi.db.meta("automation_v2").unwrap().unwrap()).unwrap();
    state["apps"]["gboard"] = state["apps"]["youtube"].clone();
    let mut job = state["jobs"][0].clone();
    job["app"] = json!("gboard");
    state["jobs"].as_array_mut().unwrap().push(job);
    k.kiwi
        .db
        .set_meta("automation_v2", &state.to_string())
        .unwrap();
    let native = k.kiwi.config.data_dir.join("automation/native");
    std::fs::create_dir_all(&native).unwrap();
    std::fs::write(native.join("gboard-rambler.json"), "{}").unwrap();

    let k = TestKiwi::start_in(k.stop().await, FakeDevices::with_phone(), false).await;
    let phone = k.phone();
    let built = phone.ok("/api/apps?source=kiwi-youtube").await;
    let bundles: Vec<&str> = built
        .as_array()
        .unwrap()
        .iter()
        .map(|a| a["bundle_id"].as_str().unwrap())
        .collect();
    assert_eq!(bundles, ["com.google.ios.youtube"], "{built}");
    assert!(
        phone
            .ok("/api/apps/kiwi-youtube/com.google.ios.youtube")
            .await["state"]["install_url"]
            .is_string()
    );
    let state: Value =
        serde_json::from_str(&k.kiwi.db.meta("automation_v2").unwrap().unwrap()).unwrap();
    assert!(state["apps"].get("gboard").is_none());
    assert!(
        state["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .all(|j| j["app"] != "gboard")
    );
    assert!(!native.exists());
}

const ANDROID_IP: &str = "100.64.0.9";

/// An Android builder: a signed APK is whatever the runner says it is, so the fixture's
/// is a few bytes with the package the recipe names.
fn fake_android_runner(k: &TestKiwi, package: &str) {
    let script = k.dir.path().join("fake-android-builder.py");
    let text = format!(
        r#"
import sys,json,pathlib,hashlib
command=sys.argv[1]
recipe=sys.argv[sys.argv.index('--recipe')+1]
root=pathlib.Path(sys.argv[sys.argv.index('--data-dir')+1])/'automation'
request=json.loads(pathlib.Path(sys.argv[sys.argv.index('--request')+1]).read_text())
def emit(kind,**data):print(json.dumps(dict(event=kind,**data)),flush=True)
if command=='plan':
 emit('health',checks=[dict(name='Fixture',ready=True)],ready=True)
 key=hashlib.sha256(recipe.encode()).hexdigest()
 emit('plan',plan=dict(app=recipe,platform='android',key=key,version='21.30.216',external_id='1561187872',tweaks=[],name=None))
else:
 data=b'PK'+recipe.encode()+bytes(str(request['sequence']),'ascii')+b'x'*300000
 sha=hashlib.sha256(data).hexdigest()
 dest=root/'artifacts'
 dest.mkdir(exist_ok=True)
 (dest/(sha+'.apk')).write_bytes(data)
 emit('artifact',sha256=sha,version='21.30.216',build='1561187872',revision=request['sequence'],min_os=None,size=len(data),package='{package}',signer='c'*64,notes='Fixture')
"#
    );
    std::fs::write(&script, text).unwrap();
    *k.kiwi.automation.android_runner.write().unwrap() = script;
}

#[tokio::test]
async fn android_apps_are_built_without_signing_and_install_as_apk_downloads() {
    const PACKAGE: &str = "app.morphe.android.youtube";
    let k = TestKiwi::start_building().await;
    fake_runner(&k, false);
    fake_android_runner(&k, PACKAGE);
    k.devices.add_android(ANDROID_IP);
    let phone = k.phone();
    let android = k.android(ANDROID_IP);

    let initial = phone.ok("/api/automation/android-youtube").await;
    assert_eq!(initial["platform"], "android");
    assert_eq!(initial["added"], false);
    assert_eq!(initial["app_version"], "21.30.216");
    let added = phone
        .post(
            "/api/automation/android-youtube/add",
            initial["preferences"].clone(),
        )
        .await;
    assert_eq!(added.status, StatusCode::ACCEPTED, "{}", added.body);
    let ready = finished_app(&k, "android-youtube").await;
    assert_eq!(ready["jobs"][0]["status"], "ready", "{ready}");
    assert_eq!(ready["jobs"][0]["stage"], "Available in Kiwi");
    assert_eq!(ready["published"]["version"], "21.30.216");
    assert_eq!(
        k.signer.signed.load(Ordering::SeqCst),
        0,
        "an Android app is signed by its builder, not with the Apple identity"
    );

    // The Android phone is recognised from its browser and its Tailscale client.
    let me = android.ok("/api/me").await;
    assert_eq!(me["device"]["model"], "Android");
    assert_eq!(me["device"]["platform"], "android");
    assert_eq!(phone.ok("/api/me").await["device"]["platform"], "ios");

    // Browse shows each device the apps made for it.
    let names = |v: Value| -> Vec<String> {
        v.as_array()
            .unwrap()
            .iter()
            .map(|a| a["bundle_id"].as_str().unwrap().to_owned())
            .collect()
    };
    phone.post("/api/automation/youtube/check", json!({})).await;
    assert_eq!(finished(&k).await["jobs"][0]["status"], "ready");
    assert_eq!(k.signer.signed.load(Ordering::SeqCst), 1);
    assert_eq!(names(android.ok("/api/apps").await), [PACKAGE]);
    assert_eq!(
        names(phone.ok("/api/apps").await),
        ["com.google.ios.youtube"]
    );
    let other = phone.ok(&format!("/api/apps/kiwi-youtube/{PACKAGE}")).await;
    assert_eq!(other["state"]["action"], "unavailable", "{other}");
    assert_eq!(other["unavailable"], "Made for Android devices");
    let other = android
        .ok("/api/apps/kiwi-youtube/com.google.ios.youtube")
        .await;
    assert_eq!(other["unavailable"], "Made for iPhone and iPad");

    // The install link is the APK itself, under the app's own package name.
    let path = format!("/api/apps/kiwi-youtube/{PACKAGE}");
    let app = android.ok(&path).await;
    assert_eq!(app["platform"], "android");
    assert_eq!(app["original_match"], "none");
    assert_eq!(
        app["state"]["action"], "install",
        "the build Kiwi Builds published is ready for every Android device: {app}"
    );
    let prepared = android.post(&format!("{path}/prepare"), json!({})).await;
    assert_eq!(prepared.status, StatusCode::ACCEPTED, "{}", prepared.body);
    assert_eq!(prepared.body["status"], "ready", "{}", prepared.body);
    assert_eq!(prepared.body["signed_bundle_id"], PACKAGE);
    assert!(prepared.body["expires_at"].is_null());
    let url = prepared.body["install_url"].as_str().unwrap();
    let ota = url.strip_prefix("https://kiwi.test").unwrap();
    assert!(
        ota.starts_with("/ota/") && ota.ends_with("/app.apk"),
        "{url}"
    );
    assert_eq!(
        android.ok(&path).await["state"]["install_url"],
        url,
        "one link per build and device"
    );

    // An APK is never served as an IPA, or the other way around.
    let token = ota
        .strip_prefix("/ota/")
        .unwrap()
        .strip_suffix("/app.apk")
        .unwrap();
    for wrong in ["app.ipa", "manifest.plist"] {
        let r = android.get(&format!("/ota/{token}/{wrong}")).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{wrong}");
    }
    let partial = android
        .request(Method::GET, ota)
        .header("Range", "bytes=0-1")
        .send()
        .await
        .unwrap();
    assert_eq!(partial.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(&partial.bytes().await.unwrap()[..], b"PK");
    assert!(android.ok(&path).await["state"]["installed"].is_null());

    let response = android.request(Method::GET, ota).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(
        response.headers()["content-type"],
        "application/vnd.android.package-archive"
    );
    assert_eq!(
        response.headers()["content-disposition"],
        "attachment; filename=\"YouTube-21.30.216.apk\""
    );
    let bytes = response.bytes().await.unwrap();
    assert!(bytes.starts_with(b"PKandroid-youtube"));

    // The whole download records the install; nothing expires.
    let mut state = Value::Null;
    for _ in 0..100 {
        state = android.ok(&path).await["state"].clone();
        if state["installed"].is_object() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(state["installed"]["version"], "21.30.216", "{state}");
    assert!(state["installed"]["expires_at"].is_null());
    let me = android.ok("/api/me").await;
    assert_eq!(me["counts"]["updates"], 0);
    assert_eq!(
        me["warnings"],
        json!([]),
        "Apple signing and the iOS version don't concern an Android phone"
    );
    assert_eq!(names(android.ok("/api/library").await), [PACKAGE]);
    assert!(names(phone.ok("/api/library").await).is_empty());

    // A rebuild is an update for the Android phone only.
    phone
        .post(
            "/api/automation/android-youtube/check",
            json!({"force":true}),
        )
        .await;
    let rebuilt = finished_app(&k, "android-youtube").await;
    assert_eq!(rebuilt["jobs"][0]["status"], "ready", "{rebuilt}");
    assert_eq!(names(android.ok("/api/updates").await), [PACKAGE]);
    assert!(names(phone.ok("/api/updates").await).is_empty());
    assert_eq!(k.signer.signed.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn android_builds_must_be_the_app_in_the_recipe() {
    let k = TestKiwi::start_building().await;
    fake_runner(&k, false);
    fake_android_runner(&k, "com.example.other");
    let phone = k.phone();
    let initial = phone.ok("/api/automation/android-youtube").await;
    phone
        .post(
            "/api/automation/android-youtube/add",
            initial["preferences"].clone(),
        )
        .await;
    let failed = finished_app(&k, "android-youtube").await;
    assert_eq!(failed["jobs"][0]["status"], "failed", "{failed}");
    assert_eq!(
        failed["jobs"][0]["error"],
        "The patched app is not the one in the recipe"
    );
    assert!(failed["published"].is_null());
    assert!(k.kiwi.db.builds().unwrap().is_empty());
}

#[tokio::test]
async fn kiwis_android_app_reports_which_packages_are_installed() {
    const PACKAGE: &str = "app.morphe.android.youtube";
    const KIWI_KEY: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
    const PLAY_KEY: &str = "dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd";
    let k = TestKiwi::start_building().await;
    fake_runner(&k, false);
    fake_android_runner(&k, PACKAGE);
    k.devices.add_android(ANDROID_IP);
    let phone = k.phone();
    let initial = phone.ok("/api/automation/android-youtube").await;
    phone
        .post(
            "/api/automation/android-youtube/add",
            initial["preferences"].clone(),
        )
        .await;
    assert_eq!(
        finished_app(&k, "android-youtube").await["jobs"][0]["status"],
        "ready"
    );
    let app = Api {
        user_agent: Some("Kiwi-Android/1.0.0 (1; Android 15; SDK 35; Google Pixel 9a)".into()),
        ..k.android(ANDROID_IP)
    };
    let report = |signer: &str| {
        json!({ "packages": [
            { "package": PACKAGE, "version_name": "21.30.216", "version_code": 1561187872,
              "signer_sha256": [signer], "installer": "com.android.packageinstaller" },
            { "package": "com.example.unrelated", "version_name": "1.0", "version_code": 1,
              "signer_sha256": [PLAY_KEY], "installer": null },
        ]})
    };
    let put = |body: Value| {
        let app = app.clone();
        async move {
            let r = app.send(Method::PUT, "/api/me/packages", Some(body)).await;
            assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.body);
        }
    };
    let path = format!("/api/apps/kiwi-youtube/{PACKAGE}");

    // Until Kiwi knows its own Android key, it can't tell whose a package is.
    put(report(PLAY_KEY)).await;
    let state = app.ok(&path).await["state"].clone();
    assert_eq!(state["foreign_install"], false);
    assert!(state["installed"].is_null());

    // The runner records the key's certificate whenever it loads the key.
    let signer = k.dir.path().join("data/automation/android/signer.json");
    std::fs::create_dir_all(signer.parent().unwrap()).unwrap();
    std::fs::write(&signer, json!({ "sha256": KIWI_KEY }).to_string()).unwrap();
    // Settings shows its fingerprint, so the owner can tell which key to back up.
    assert_eq!(
        app.ok("/api/me").await["host"]["android_signer_sha256"],
        KIWI_KEY
    );

    // The same package from another publisher: Kiwi's build can't be installed over it.
    put(report(PLAY_KEY)).await;
    let state = app.ok(&path).await["state"].clone();
    assert_eq!(state["foreign_install"], true, "{state}");
    assert!(state["installed"].is_null());
    assert_eq!(state["action"], "install");

    // Kiwi's build, installed outside Kiwi's download: a verified install.
    put(report(&KIWI_KEY.to_uppercase())).await;
    let state = app.ok(&path).await["state"].clone();
    assert_eq!(state["foreign_install"], false);
    assert_eq!(state["action"], "open", "{state}");
    assert_eq!(state["installed"]["verified"], true);
    assert_eq!(state["installed"]["origin"], "device");
    assert_eq!(state["installed"]["version"], "21.30.216");
    assert_eq!(app.ok("/api/me").await["counts"]["installed"], 1);

    // Uninstalled on the phone: the install is marked removed.
    put(json!({ "packages": [] })).await;
    let state = app.ok(&path).await["state"].clone();
    assert!(state["installed"].is_null(), "{state}");
    assert!(app.ok("/api/library").await.as_array().unwrap().is_empty());
    assert_eq!(app.ok("/api/me").await["counts"]["installed"], 0);
}

#[tokio::test]
async fn an_android_install_missing_after_the_grace_period_is_removed() {
    const PACKAGE: &str = "app.morphe.android.youtube";
    let k = TestKiwi::start_building().await;
    fake_runner(&k, false);
    fake_android_runner(&k, PACKAGE);
    k.devices.add_android(ANDROID_IP);
    let phone = k.phone();
    let initial = phone.ok("/api/automation/android-youtube").await;
    phone
        .post(
            "/api/automation/android-youtube/add",
            initial["preferences"].clone(),
        )
        .await;
    assert_eq!(
        finished_app(&k, "android-youtube").await["jobs"][0]["status"],
        "ready"
    );
    let signer = k.dir.path().join("data/automation/android/signer.json");
    std::fs::create_dir_all(signer.parent().unwrap()).unwrap();
    std::fs::write(&signer, json!({ "sha256": "c".repeat(64) }).to_string()).unwrap();
    let app = Api {
        user_agent: Some("Kiwi-Android/1.0.0 (1; Android 15; SDK 35; Google Pixel 9a)".into()),
        ..k.android(ANDROID_IP)
    };
    let path = format!("/api/apps/kiwi-youtube/{PACKAGE}");
    let prepared = app.post(&format!("{path}/prepare"), json!({})).await;
    let url = prepared.body["install_url"].as_str().unwrap();
    let ota = url.strip_prefix("https://kiwi.test").unwrap();
    let response = app.request(Method::GET, ota).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.bytes().await.unwrap();
    let mut state = Value::Null;
    for _ in 0..100 {
        state = app.ok(&path).await["state"].clone();
        if state["installed"].is_object() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(state["installed"]["origin"], "kiwi", "{state}");

    // Android's install fails after the download; the app reports at once, without the
    // package. Android may still be installing it, so it stays recorded for now...
    let r = app
        .send(
            Method::PUT,
            "/api/me/packages",
            Some(json!({ "packages": [] })),
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.body);
    let now = chrono::Utc::now();
    kiwi_server::android::settle_reports(&k.kiwi, now).unwrap();
    assert!(app.ok(&path).await["state"]["installed"].is_object());
    // Kiwi keeps the report for that install alone (it lists none of its packages).
    let android_id = app.ok("/api/me").await["device"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_eq!(k.kiwi.package_reports.kept(), [(android_id, 0)]);
    // ...until the grace period is over: then that report counts, without another one.
    kiwi_server::android::settle_reports(&k.kiwi, now + chrono::Duration::minutes(11)).unwrap();
    let state = app.ok(&path).await["state"].clone();
    assert!(state["installed"].is_null(), "{state}");
    assert_eq!(state["action"], "install");
    assert_eq!(app.ok("/api/me").await["counts"]["installed"], 0);
    // Nothing waits for the report any more.
    assert!(k.kiwi.package_reports.kept().is_empty());
}

#[tokio::test]
async fn a_browser_download_is_not_removed_by_a_report_made_before_its_install() {
    const PACKAGE: &str = "app.morphe.android.youtube";
    let k = TestKiwi::start_building().await;
    fake_runner(&k, false);
    fake_android_runner(&k, PACKAGE);
    k.devices.add_android(ANDROID_IP);
    let phone = k.phone();
    let initial = phone.ok("/api/automation/android-youtube").await;
    phone
        .post(
            "/api/automation/android-youtube/add",
            initial["preferences"].clone(),
        )
        .await;
    assert_eq!(
        finished_app(&k, "android-youtube").await["jobs"][0]["status"],
        "ready"
    );
    let signer = k.dir.path().join("data/automation/android/signer.json");
    std::fs::create_dir_all(signer.parent().unwrap()).unwrap();
    std::fs::write(&signer, json!({ "sha256": "c".repeat(64) }).to_string()).unwrap();
    // The owner downloads the APK in Chrome...
    let browser = k.android(ANDROID_IP);
    let path = format!("/api/apps/kiwi-youtube/{PACKAGE}");
    let prepared = browser.post(&format!("{path}/prepare"), json!({})).await;
    let url = prepared.body["install_url"].as_str().unwrap();
    let ota = url.strip_prefix("https://kiwi.test").unwrap();
    let response = browser.request(Method::GET, ota).send().await.unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.bytes().await.unwrap();
    let mut state = Value::Null;
    for _ in 0..100 {
        state = browser.ok(&path).await["state"].clone();
        if state["installed"].is_object() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    assert_eq!(state["installed"]["origin"], "kiwi", "{state}");

    // ...and Kiwi's app reports the packages before they open it in Android's installer.
    let app = Api {
        user_agent: Some("Kiwi-Android/1.0.0 (1; Android 15; SDK 35; Google Pixel 9a)".into()),
        ..k.android(ANDROID_IP)
    };
    let r = app
        .send(
            Method::PUT,
            "/api/me/packages",
            Some(json!({ "packages": [] })),
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.body);
    // That report says nothing about the download, so Kiwi doesn't keep it...
    assert!(k.kiwi.package_reports.kept().is_empty());
    // ...and the install stands after the grace period.
    let now = chrono::Utc::now();
    kiwi_server::android::settle_reports(&k.kiwi, now + chrono::Duration::minutes(11)).unwrap();
    assert!(app.ok(&path).await["state"]["installed"].is_object());

    // A report made after the grace period decides, as for any install.
    let device = app.ok("/api/me").await["device"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    k.kiwi
        .db
        .change_installs(&device, |installs| {
            installs
                .iter()
                .map(|i| kiwi_server::db::InstallRow {
                    installed_at: i.installed_at - chrono::Duration::minutes(11),
                    ..i.clone()
                })
                .collect()
        })
        .unwrap();
    let r = app
        .send(
            Method::PUT,
            "/api/me/packages",
            Some(json!({ "packages": [] })),
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.body);
    assert!(app.ok(&path).await["state"]["installed"].is_null());
}

/// The Android runner's `kiwi-app` command, as far as Kiwi sees it: writes the signed app
/// and its description for the input it is given, or fails. Counts its runs.
fn fake_kiwi_app_runner(k: &TestKiwi, fail: bool) {
    let script = k.dir.path().join("fake-kiwi-app.py");
    let fail = if fail { "True" } else { "False" };
    let text = format!(
        r#"
import sys,json,pathlib,hashlib
assert sys.argv[1]=='kiwi-app' and '--recipe' not in sys.argv
data=pathlib.Path(sys.argv[sys.argv.index('--data-dir')+1])
request=json.loads(pathlib.Path(sys.argv[sys.argv.index('--request')+1]).read_text())
def emit(kind,**d):print(json.dumps(dict(event=kind,**d)),flush=True)
runs=data/'kiwi-app-runs'
runs.write_text(str(int(runs.read_text())+1 if runs.exists() else 1))
if {fail}:
 emit('error',message="Kiwi could not fetch its Android app 1.2.0: it could not be reached.",blocked=False)
 sys.exit(1)
emit('stage',stage='Signing Kiwi for Android 1.2.0')
apk=b'PK signed '+request['input'].encode()
sha=hashlib.sha256(apk).hexdigest()
out=data/'android-app'
out.mkdir(exist_ok=True)
(out/'kiwi.apk').write_bytes(apk)
info=dict(package='io.github.digitalpals.kiwi',version_name='1.2.0',version_code=1002000,sha256=sha,size=len(apk))
(out/'kiwi.json').write_text(json.dumps(dict(info,signer='c'*64,source='local',input=request['input'])))
emit('kiwi_app',source='local',**info)
"#
    );
    std::fs::write(&script, text).unwrap();
    *k.kiwi.automation.android_runner.write().unwrap() = script;
}

#[tokio::test]
async fn kiwi_signs_and_offers_its_own_android_app() {
    let k = TestKiwi::start_building().await;
    k.devices.add_android(ANDROID_IP);
    fake_kiwi_app_runner(&k, false);
    let data = k.dir.path().join("data");
    let runs = || {
        std::fs::read_to_string(data.join("kiwi-app-runs"))
            .map_or(0, |n| n.trim().parse::<u32>().unwrap())
    };
    let settle = || async {
        // The job runs on Kiwi Builds' worker; wait until it is free again.
        for _ in 0..200 {
            tokio::time::sleep(std::time::Duration::from_millis(25)).await;
            if k.kiwi.automation.kiwi_app.lock().unwrap().is_idle() {
                return;
            }
        }
        panic!("the kiwi-app job did not finish");
    };
    let unsigned = data.join("android-app/unsigned");
    std::fs::create_dir_all(&unsigned).unwrap();
    std::fs::write(unsigned.join("kiwi-1.2.0.apk"), "PK unsigned").unwrap();

    // Without an Android device or Android build, Kiwi has no one to offer its app to.
    kiwi_server::kiwi_app::tick(&k.kiwi).await;
    settle().await;
    assert_eq!(runs(), 0);
    // Nor for someone else's Android phone, refused when it opened Kiwi.
    k.devices.peers.lock().unwrap().insert(
        "100.64.0.30".parse().unwrap(),
        kiwi_device::TailscalePeer {
            stable_id: "nNOAHPIXEL".into(),
            name: "noah-pixel".into(),
            dns_name: "noah-pixel.example.ts.net".into(),
            os: Some("android".into()),
            user_login: Some("noah@example.com".into()),
            user_name: None,
        },
    );
    let noah = k.android("100.64.0.30").get("/api/me").await;
    assert_eq!(noah.status, StatusCode::FORBIDDEN);
    assert!(k.kiwi.db.device("ts:nNOAHPIXEL").unwrap().is_some());
    kiwi_server::kiwi_app::tick(&k.kiwi).await;
    settle().await;
    assert_eq!(runs(), 0);

    // The Android phone appears: Kiwi signs the app and offers it.
    let android = k.android(ANDROID_IP);
    assert!(android.ok("/api/me").await["android_app"].is_null());
    kiwi_server::kiwi_app::tick(&k.kiwi).await;
    settle().await;
    assert_eq!(runs(), 1);
    let me = android.ok("/api/me").await;
    assert_eq!(me["android_app"]["version_name"], "1.2.0", "{me}");
    assert_eq!(me["android_app"]["package"], "io.github.digitalpals.kiwi");
    assert!(
        me["features"]
            .as_array()
            .unwrap()
            .contains(&json!("android_app_offer"))
    );
    let apk = android.get("/android/kiwi.apk").await;
    assert_eq!(apk.status, StatusCode::OK);
    assert!(!data.join("automation/kiwi-app.request.json").exists());

    // Nothing changed: nothing runs.
    kiwi_server::kiwi_app::tick(&k.kiwi).await;
    settle().await;
    assert_eq!(runs(), 1);

    // A new unsigned build in the folder is signed and offered.
    std::fs::write(unsigned.join("kiwi-1.2.1.apk"), "PK newer").unwrap();
    kiwi_server::kiwi_app::tick(&k.kiwi).await;
    settle().await;
    assert_eq!(runs(), 2);

    // A failure is logged and shown on Android devices; it is not retried at once, and the
    // app that was signed before stays offered.
    fake_kiwi_app_runner(&k, true);
    std::fs::write(unsigned.join("kiwi-1.2.2.apk"), "PK newest").unwrap();
    kiwi_server::kiwi_app::tick(&k.kiwi).await;
    settle().await;
    assert_eq!(runs(), 3);
    let me = android.ok("/api/me").await;
    let warning = me["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .find(|w| w["code"] == "android_app")
        .unwrap_or_else(|| panic!("{me}"))
        .clone();
    assert_eq!(
        warning["message"],
        "Kiwi couldn't prepare its Android app: Kiwi could not fetch its Android app 1.2.0: it could not be reached."
    );
    assert_eq!(warning["level"], "warning");
    assert!(!me["android_app"].is_null());
    let phone = k.phone();
    assert!(
        !phone.ok("/api/me").await["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["code"] == "android_app"),
        "an iPhone has no use for Kiwi's Android app"
    );
    kiwi_server::kiwi_app::tick(&k.kiwi).await;
    settle().await;
    assert_eq!(runs(), 3);

    // With Kiwi Builds off nothing runs, and the warning is gone.
    let r = phone
        .send(
            Method::PATCH,
            "/api/settings",
            Some(json!({"kiwi_builds": false})),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert!(
        !android.ok("/api/me").await["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["code"] == "android_app")
    );
    std::fs::write(unsigned.join("kiwi-1.2.3.apk"), "PK even newer").unwrap();
    kiwi_server::kiwi_app::tick(&k.kiwi).await;
    settle().await;
    assert_eq!(runs(), 3);
}

/// The whole delivery with the real Android runner: Kiwi notices an unsigned APK of its app,
/// runs `android.py kiwi-app` on the worker (which fetches the pinned Java runtime and
/// build-tools and creates Kiwi's key), checks the result and serves it. Run by hand; it
/// needs internet access and an unsigned release APK:
///
/// ```sh
/// KIWI_TEST_UNSIGNED_APK=$PWD/android/app/build/outputs/apk/release/app-release-unsigned.apk \
///   cargo test -p kiwi-server --test automation -- --ignored --nocapture real_kiwi_app
/// ```
///
/// `KIWI_TEST_SIGNED_APK=<path>` keeps a copy of the signed app.
#[tokio::test]
#[ignore = "needs internet access and KIWI_TEST_UNSIGNED_APK"]
async fn real_kiwi_app_runner_signs_and_serves_an_unsigned_apk() {
    use sha2::{Digest, Sha256};
    let source = std::env::var("KIWI_TEST_UNSIGNED_APK").expect("KIWI_TEST_UNSIGNED_APK");
    let k = TestKiwi::start_building().await;
    k.devices.add_android(ANDROID_IP);
    let android = k.android(ANDROID_IP);
    assert!(android.ok("/api/me").await["android_app"].is_null());
    let data = k.dir.path().join("data");
    let unsigned = data.join("android-app/unsigned");
    std::fs::create_dir_all(&unsigned).unwrap();
    std::fs::copy(&source, unsigned.join("kiwi.apk")).unwrap();

    kiwi_server::kiwi_app::tick(&k.kiwi).await;
    for _ in 0..1200 {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        if k.kiwi.automation.kiwi_app.lock().unwrap().is_idle() {
            break;
        }
    }
    let log = std::fs::read_to_string(data.join("automation/kiwi-app.log")).unwrap_or_default();
    if let Some(failure) = kiwi_server::kiwi_app::failure(&k.kiwi) {
        panic!("{}\n{log}", failure.message);
    }
    let me = android.ok("/api/me").await;
    let offered = &me["android_app"];
    assert_eq!(offered["package"], "io.github.digitalpals.kiwi", "{me}");
    println!("offered: {offered}");

    let meta: Value =
        serde_json::from_slice(&std::fs::read(data.join("android-app/kiwi.json")).unwrap())
            .unwrap();
    let response = android
        .request(Method::GET, "/android/kiwi.apk")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = response.bytes().await.unwrap();
    assert_eq!(bytes.len() as u64, meta["size"].as_u64().unwrap());
    assert_eq!(hex::encode(Sha256::digest(&bytes)), meta["sha256"]);
    assert_eq!(meta["source"], "local");
    let signer: Value = serde_json::from_slice(
        &std::fs::read(data.join("automation/android/signer.json")).unwrap(),
    )
    .unwrap();
    assert_eq!(meta["signer"], signer["sha256"]);
    // Up to date now: the next check runs nothing.
    let log_size = log.len();
    kiwi_server::kiwi_app::tick(&k.kiwi).await;
    assert!(k.kiwi.automation.kiwi_app.lock().unwrap().is_idle());
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;
    let log = std::fs::read_to_string(data.join("automation/kiwi-app.log")).unwrap();
    assert_eq!(log.len(), log_size);
    println!("kiwi.json: {meta}\nrunner log:\n{log}");
    if let Ok(keep) = std::env::var("KIWI_TEST_SIGNED_APK") {
        std::fs::write(&keep, &bytes).unwrap();
        println!("signed app kept at {keep}");
    }
}

#[tokio::test]
async fn builds_someone_asked_for_go_first_show_their_place_and_adding_twice_joins() {
    let k = TestKiwi::start_building().await;
    fake_runner(&k, false);
    fake_android_runner(&k, "app.morphe.android.youtube");
    let phone = k.phone();

    // YouTube holds the worker in signing.
    k.signer.hold.store(true, Ordering::SeqCst);
    phone.post("/api/automation/youtube/check", json!({})).await;
    wait_until(|| !k.kiwi.db.builds().unwrap().is_empty()).await;

    // Twitch is added (and its job cancelled), then queued by the schedule.
    let twitch = json!({"enabled":true,"interval_minutes":360,"tweaks":[]});
    phone
        .post("/api/automation/twitch/add", twitch.clone())
        .await;
    phone.post("/api/automation/twitch/cancel", json!({})).await;
    kiwi_server::automation::queue(&k.kiwi, "twitch", false, false).unwrap();
    let scheduled = phone.ok("/api/automation/twitch").await;
    assert_eq!(scheduled["jobs"][0]["status"], "queued");
    assert_eq!(scheduled["jobs"][0]["ahead"], 1, "{scheduled}");
    assert!(scheduled["jobs"][0].get("interactive").is_none());

    // A tap on Install comes later but goes before the scheduled build.
    let preferences = phone.ok("/api/automation/android-youtube").await["preferences"].clone();
    let added = phone
        .post("/api/automation/android-youtube/add", preferences.clone())
        .await;
    assert_eq!(added.status, StatusCode::ACCEPTED, "{}", added.body);
    assert_eq!(added.body["jobs"][0]["status"], "queued");
    assert_eq!(added.body["jobs"][0]["ahead"], 1, "{}", added.body);
    assert_eq!(added.body["jobs"][0]["interactive"], true);
    let job = added.body["jobs"][0]["id"].clone();
    let overview = phone.ok("/api/automation").await;
    let twitch_row = overview["apps"]
        .as_array()
        .unwrap()
        .iter()
        .find(|a| a["id"] == "twitch")
        .unwrap();
    assert_eq!(twitch_row["jobs"][0]["ahead"], 2, "{twitch_row}");

    // A second tap joins the job instead of failing or queueing another.
    let again = phone
        .post("/api/automation/android-youtube/add", preferences.clone())
        .await;
    assert_eq!(again.status, StatusCode::ACCEPTED, "{}", again.body);
    assert_eq!(again.body["jobs"][0]["id"], job);
    assert_eq!(again.body["jobs"].as_array().unwrap().len(), 1);
    // Other choices while it waits are a conflict.
    let mut other = preferences.clone();
    other["interval_minutes"] = json!(1440);
    let conflict = phone
        .post("/api/automation/android-youtube/add", other)
        .await;
    assert_eq!(conflict.status, StatusCode::CONFLICT, "{}", conflict.body);

    // The scheduled Twitch job moves ahead when someone asks for it too; it was queued first.
    let joined = phone.post("/api/automation/twitch/add", twitch).await;
    assert_eq!(joined.status, StatusCode::ACCEPTED, "{}", joined.body);
    assert_eq!(joined.body["jobs"][0]["interactive"], true);
    assert_eq!(joined.body["jobs"][0]["ahead"], 1, "{}", joined.body);
    assert_eq!(
        phone.ok("/api/automation/android-youtube").await["jobs"][0]["ahead"],
        2
    );

    k.signer.hold.store(false, Ordering::SeqCst);
    let android = finished_app(&k, "android-youtube").await;
    let twitch = finished_app(&k, "twitch").await;
    assert_eq!(android["jobs"][0]["status"], "ready", "{android}");
    assert_eq!(twitch["jobs"][0]["status"], "ready", "{twitch}");
    assert!(android["jobs"][0].get("ahead").is_none());
    let finished = |v: &Value| {
        chrono::DateTime::parse_from_rfc3339(v["jobs"][0]["finished_at"].as_str().unwrap()).unwrap()
    };
    assert!(finished(&twitch) <= finished(&android));
}

#[tokio::test]
async fn scheduled_jobs_wait_behind_jobs_someone_asked_for() {
    let k = TestKiwi::start_building().await;
    fake_runner(&k, false);
    let phone = k.phone();
    k.signer.hold.store(true, Ordering::SeqCst);
    phone.post("/api/automation/youtube/check", json!({})).await;
    wait_until(|| !k.kiwi.db.builds().unwrap().is_empty()).await;
    let prefs = json!({"enabled":true,"interval_minutes":360,"tweaks":[]});
    // Reddit is queued by the schedule first, X by a tap afterwards.
    phone
        .post("/api/automation/reddit/add", prefs.clone())
        .await;
    phone.post("/api/automation/reddit/cancel", json!({})).await;
    kiwi_server::automation::queue(&k.kiwi, "reddit", false, false).unwrap();
    phone.post("/api/automation/x/add", prefs).await;
    k.signer.hold.store(false, Ordering::SeqCst);
    let x = finished_app(&k, "x").await;
    let reddit = finished_app(&k, "reddit").await;
    assert_eq!(x["jobs"][0]["status"], "ready", "{x}");
    assert_eq!(reddit["jobs"][0]["status"], "ready", "{reddit}");
    // Builds run one at a time, so the order they finished in is the order they started in.
    let finished = |v: &Value| {
        chrono::DateTime::parse_from_rfc3339(v["jobs"][0]["finished_at"].as_str().unwrap()).unwrap()
    };
    assert!(finished(&x) <= finished(&reddit), "x {x} reddit {reddit}");
}
