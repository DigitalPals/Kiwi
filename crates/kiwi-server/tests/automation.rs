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
