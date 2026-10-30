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
bundle=dict(youtube='com.google.ios.youtube',twitch='tv.twitch',x='com.atebits.Tweetie2',reddit='com.reddit.Reddit',rambler='com.kiwi.ramblerpoc',gboard='com.google.keyboard')[recipe]
version='0.1.0' if recipe=='rambler' else ('2.3.19' if recipe=='gboard' else ('2026.43.0' if recipe=='reddit' else '21.44.1'))
root=pathlib.Path(sys.argv[sys.argv.index('--data-dir')+1])/'automation'
request=json.loads(pathlib.Path(sys.argv[sys.argv.index('--request')+1]).read_text())
def emit(kind,**data):print(json.dumps(dict(event=kind,**data)),flush=True)
if command=='plan':
 emit('health',checks=[dict(name='Native iOS build tools' if recipe=='rambler' else 'Fixture',ready=True)],ready=True)
 {pause}
 key='a'*64
 if sorted(request['tweaks'])!=sorted(['youpip','ryd','ytabconfig','youquality']):
  key=hashlib.sha256(json.dumps(sorted(request['tweaks'])).encode()).hexdigest()
 if recipe!='youtube':
  key=hashlib.sha256(json.dumps([recipe,request['tweaks'],request.get('name')]).encode()).hexdigest()
 emit('plan',plan=dict(key=key,version=version,external_id='native-'+key[:16] if recipe=='rambler' else '1234',tweaks=request['tweaks'],name=request.get('name')))
else:
 number='1' if recipe=='rambler' else ('2.3.19.444584732' if recipe=='gboard' else version)
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
            "rambler",
            "gboard"
        ]
    );
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
async fn native_rambler_publishes_through_normal_signing_without_overwriting_app_store_checks() {
    let k = TestKiwi::start_building().await;
    fake_runner(&k, false);
    let phone = k.phone();
    let manifest = k
        .kiwi
        .config
        .data_dir
        .join("automation/native/rambler.json");
    let files: Value = serde_json::from_slice(&std::fs::read(manifest).unwrap()).unwrap();
    for name in [
        "build.sh",
        "make-sample.sh",
        "extract-caller.py",
        "Sources/App.swift",
        "Sources/CLI.swift",
        "Sources/RamblerCore.swift",
        "Sources/OggOpus.swift",
        "Tests/ProtocolTests.swift",
    ] {
        assert!(
            files[name].as_str().is_some_and(|s| !s.is_empty()),
            "missing {name}"
        );
    }
    assert!(files["caller.json"].is_null());

    phone.post("/api/automation/youtube/check", json!({})).await;
    assert_eq!(finished(&k).await["jobs"][0]["status"], "ready");
    let shared = phone.ok("/api/automation").await["checks"].clone();
    assert!(!shared.as_array().unwrap().is_empty());
    let initial = phone.ok("/api/automation/rambler").await;
    assert_eq!(initial["experimental"], true);
    assert_eq!(initial["added"], false);
    assert_eq!(initial["checks"], json!([]));
    assert_eq!(initial["included"], json!([]));
    assert_eq!(initial["available_tweaks"], json!([]));
    assert_eq!(initial["preferences"]["tweaks"], json!([]));
    let added = phone
        .post(
            "/api/automation/rambler/add",
            initial["preferences"].clone(),
        )
        .await;
    assert_eq!(added.status, StatusCode::ACCEPTED, "{}", added.body);
    let ready = finished_app(&k, "rambler").await;
    assert_eq!(ready["jobs"][0]["status"], "ready", "{ready}");
    assert_eq!(ready["published"]["version"], "0.1.0");
    assert_eq!(ready["published"]["build"], "1");
    assert_eq!(ready["published"]["revision"], 1);
    assert!(
        ready["jobs"][0]["external_id"]
            .as_str()
            .unwrap()
            .starts_with("native-")
    );
    assert_eq!(
        ready["checks"],
        json!([{"name":"Native iOS build tools","ready":true}])
    );
    assert_eq!(phone.ok("/api/automation").await["checks"], shared);
    assert_eq!(phone.ok("/api/automation/youtube").await["checks"], shared);

    let path = "/api/apps/kiwi-youtube/com.kiwi.ramblerpoc";
    let app = phone.ok(path).await;
    assert_eq!(app["name"], "Rambler PoC");
    assert!(app["state"]["install_url"].is_string());
    let prepared = phone.post(&format!("{path}/prepare"), json!({})).await;
    assert_eq!(prepared.status, StatusCode::ACCEPTED, "{}", prepared.body);
    assert_eq!(prepared.body["id"], ready["jobs"][0]["signed_build_id"]);
    assert_eq!(k.signer.signed.load(Ordering::SeqCst), 2);
    phone.post("/api/automation/rambler/check", json!({})).await;
    assert_eq!(
        finished_app(&k, "rambler").await["jobs"][0]["status"],
        "unchanged"
    );
    assert_eq!(k.signer.signed.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn gboard_integration_sources_are_shipped_and_publish_as_an_experimental_build() {
    let k = TestKiwi::start_building().await;
    fake_runner(&k, false);
    let phone = k.phone();
    let manifest = k
        .kiwi
        .config
        .data_dir
        .join("automation/native/gboard-rambler.json");
    let files: Value = serde_json::from_slice(&std::fs::read(manifest).unwrap()).unwrap();
    for name in [
        "build-gboard.sh",
        "inject-gboard.py",
        "Gboard/Bridge.h",
        "Gboard/Hooks.m",
        "Gboard/Session.swift",
        "Sources/RamblerCore.swift",
    ] {
        assert!(
            files[name].as_str().is_some_and(|s| !s.is_empty()),
            "missing {name}"
        );
    }
    assert!(files["caller.json"].is_null());
    let initial = phone.ok("/api/automation/gboard").await;
    assert_eq!(initial["experimental"], true);
    assert_eq!(initial["app_version"], "2.3.19");
    assert_eq!(initial["preferences"]["tweaks"], json!([]));
    let added = phone
        .post("/api/automation/gboard/add", initial["preferences"].clone())
        .await;
    assert_eq!(added.status, StatusCode::ACCEPTED, "{}", added.body);
    let ready = finished_app(&k, "gboard").await;
    assert_eq!(ready["jobs"][0]["status"], "ready", "{ready}");
    assert_eq!(ready["published"]["version"], "2.3.19");
    assert_eq!(ready["published"]["build"], "2.3.19.444584732");
    let app = phone.ok("/api/apps/kiwi-youtube/com.google.keyboard").await;
    assert_eq!(app["name"], "Gboard Rambler");
    assert!(app["state"]["install_url"].is_string());
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
