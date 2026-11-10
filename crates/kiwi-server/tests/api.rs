mod common;

use std::sync::atomic::Ordering;

use common::*;
use kiwi_device::DeviceApp;
use kiwi_push::Outcome;
use kiwi_server::{
    devices,
    push::{self, Payload, SendError},
    scheduler::{self, Timers},
    settings::Settings,
};
use reqwest::{Method, StatusCode, header};
use serde_json::{Value, json};

#[tokio::test]
async fn build_polling_reads_only_its_build_and_reuses_device_tokens() {
    let fx = Fixture::start().await;
    let k = TestKiwi::start().await;
    let phone = k.phone();
    let source = add_source(&phone, &fx).await;
    let prepared = phone
        .post(&format!("/api/apps/{source}/{BUNDLE}/prepare"), json!({}))
        .await;
    assert_eq!(prepared.status, StatusCode::ACCEPTED);
    let id = prepared.body["id"].as_str().unwrap();
    let ready = wait_build(&phone, id).await;
    assert_eq!(ready["status"], "ready");

    // An unrelated historical build must never be deserialized by this poll. A
    // malformed release makes any accidental full-history read observable.
    let mut unrelated = k.kiwi.db.build(id).unwrap().unwrap();
    unrelated.id = "unrelated-history".into();
    k.kiwi.db.save_build(&unrelated).unwrap();
    let conn = rusqlite::Connection::open(&k.kiwi.paths.db).unwrap();
    conn.execute(
        "UPDATE builds SET release = 'invalid JSON' WHERE id = ?1",
        [&unrelated.id],
    )
    .unwrap();
    drop(conn);

    let path = format!("/api/builds/{id}");
    assert_eq!(phone.ok(&path).await["install_url"], ready["install_url"]);
    assert_eq!(phone.ok(&path).await["install_url"], ready["install_url"]);
    // Another of the owner's devices gets its own token.
    k.devices
        .add_peer("100.64.0.9", "nIPAD1CNTRL", "alex@example.com", "iPad");
    let other = k.device("100.64.0.9").ok(&path).await;
    assert!(other["install_url"].is_string());
    assert_ne!(other["install_url"], ready["install_url"]);
    assert_eq!(
        phone.get("/api/builds/absent").await.status,
        StatusCode::NOT_FOUND
    );

    let mut expired = k.kiwi.db.build(id).unwrap().unwrap();
    expired.expires_at = Some(chrono::Utc::now() - chrono::Duration::seconds(1));
    k.kiwi.db.save_build(&expired).unwrap();
    assert!(phone.ok(&path).await["install_url"].is_null());
}

#[tokio::test]
async fn legacy_multi_app_source_can_be_added_browsed_and_refreshed() {
    let fx = Fixture::start().await;
    *fx.feed.lock().unwrap() = serde_json::from_slice(include_bytes!(
        "../../kiwi-source/tests/fixtures/driftywinds.json"
    ))
    .unwrap();
    let k = TestKiwi::start().await;
    let phone = k.phone();
    let added = phone
        .post("/api/sources", json!({ "url": fx.feed_url() }))
        .await;
    assert_eq!(added.status, StatusCode::CREATED);
    assert_eq!(added.body["name"], "driftywinds' AltStore Repo");
    assert_eq!(added.body["url"], fx.feed_url());
    assert_eq!(added.body["app_count"], 10);
    assert_eq!(added.body["diagnostics"], json!([]));
    let source = added.body["id"].as_str().unwrap();
    let apps = phone.ok(&format!("/api/apps?source={source}")).await;
    assert_eq!(apps.as_array().unwrap().len(), 10);
    let path = format!("/api/apps/{source}/com.google.ios.youtube");
    let detail = phone.ok(&path).await;
    assert_eq!(detail["state"]["action"], "get");
    assert!(
        detail["description"]
            .as_str()
            .unwrap()
            .contains("A modified version of Youtube")
    );
    assert_eq!(detail["releases"][0]["version"], "21.43.5");
    assert_eq!(detail["releases"][0]["date"], "2026-10-29");
    assert_eq!(
        detail["releases"][0]["notes"],
        "• Updated YouTube to 21.43.5\n • Tweak is using 2.0.0"
    );
    {
        let mut feed = fx.feed.lock().unwrap();
        feed["apps"][0]["version"] = json!("21.44.1");
        feed["apps"][0]["versionDescription"] = json!("Updated YouTube to 21.44.1");
    }
    let refreshed = phone
        .post(&format!("/api/sources/{source}/refresh"), json!({}))
        .await;
    assert_eq!(refreshed.status, StatusCode::OK);
    assert_eq!(refreshed.body["diagnostics"], json!([]));
    let detail = phone.ok(&path).await;
    assert_eq!(detail["releases"][0]["version"], "21.44.1");
    assert_eq!(detail["releases"][0]["notes"], "Updated YouTube to 21.44.1");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn add_browse_prepare_install_update_forget() {
    let fx = Fixture::start().await;
    let k = TestKiwi::start().await;
    let phone = k.phone();

    let health = phone.ok("/api/health").await;
    assert_eq!(health["ok"], true);
    assert_eq!(health["version"], env!("CARGO_PKG_VERSION"));
    assert_eq!(health["pid"], std::process::id());

    let me = phone.ok("/api/me").await;
    assert_eq!(me["device"]["id"], PHONE_ID);
    assert_eq!(me["device"]["kind"], "tailscale");
    assert_eq!(me["device"]["name"], "iphone194");
    assert_eq!(me["device"]["os_version"], "27.0");
    assert_eq!(me["device"]["os_version_source"], "safari");
    assert_eq!(me["server"]["team_id"], TEAM);
    assert_eq!(me["server"]["public_url"], "https://kiwi.test");
    assert_eq!(me["counts"]["sources"], 0);
    assert_eq!(me["push"]["vapid_public_key"].as_str().unwrap().len(), 87);

    // Add the source; adding it again conflicts.
    let source = add_source(&phone, &fx).await;
    let dup = phone
        .post("/api/sources", json!({ "url": fx.feed_url() }))
        .await;
    assert_eq!(dup.status, StatusCode::CONFLICT);
    assert_eq!(dup.body["error"]["code"], "conflict");
    let sources = phone.ok("/api/sources").await;
    assert_eq!(sources[0]["name"], "Test Source");
    assert_eq!(sources[0]["app_count"], 1);
    assert_eq!(sources[0]["enabled"], true);
    assert!(sources[0]["last_success_at"].is_string());

    // Browse and detail.
    let apps = phone.ok("/api/apps").await;
    assert_eq!(apps.as_array().unwrap().len(), 1);
    let app = &apps[0];
    assert_eq!(app["source_id"], source.as_str());
    assert_eq!(app["source_name"], "Test Source");
    assert_eq!(app["tint_color"], "#33AA33");
    assert_eq!(app["orphaned"], false);
    assert_eq!(app["state"]["action"], "get");
    assert_eq!(app["state"]["target"]["version"], "1.0");
    assert_eq!(app["state"]["target"]["compatible"], true);
    assert!(
        phone
            .ok("/api/apps?q=nothing")
            .await
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        phone
            .ok("/api/apps?q=kiwi%20test")
            .await
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        phone
            .ok(&format!("/api/apps?source={source}"))
            .await
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let detail = phone.ok(&format!("/api/apps/{source}/{BUNDLE}")).await;
    assert_eq!(detail["signed_bundle_id"], signed_id());
    assert_eq!(detail["releases"][0]["notes"], "Release 1.0");
    assert_eq!(
        detail["permissions"]["privacy"][0]["key"],
        "NSCameraUsageDescription"
    );
    assert_eq!(
        detail["permissions"]["privacy"][0]["description"],
        "Scan codes"
    );
    assert_eq!(
        detail["dropped_entitlements"],
        json!(["com.apple.security.application-groups"])
    );
    assert_eq!(detail["source_url"], fx.feed_url());
    let missing = phone.get(&format!("/api/apps/{source}/com.nope")).await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);
    assert_eq!(missing.body["error"]["code"], "not_found");

    // Prepare: preparing while the signer is held, then ready.
    k.signer.hold.store(true, Ordering::SeqCst);
    let prepared = phone
        .post(&format!("/api/apps/{source}/{BUNDLE}/prepare"), json!({}))
        .await;
    assert_eq!(prepared.status, StatusCode::ACCEPTED, "{}", prepared.body);
    let build_id = prepared.body["id"].as_str().unwrap().to_owned();
    assert_eq!(prepared.body["signed_bundle_id"], signed_id());
    assert!(prepared.body["install_url"].is_null());
    let mut state = app_state(&phone, &source).await;
    for _ in 0..200 {
        if state["build"]["status"] == "signing" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        state = app_state(&phone, &source).await;
    }
    assert_eq!(state["action"], "preparing");
    assert_eq!(state["build"]["status"], "signing");
    assert_eq!(phone.ok("/api/me").await["counts"]["preparing"], 1);
    k.signer.hold.store(false, Ordering::SeqCst);
    let build = wait_build(&phone, &build_id).await;
    assert_eq!(build["status"], "ready", "{build}");
    assert_eq!(build["progress"], 1.0);
    assert!(build["size"].as_u64().unwrap() > 0);
    let install_url = build["install_url"].as_str().unwrap().to_owned();
    assert!(install_url.starts_with(
        "itms-services://?action=download-manifest&url=https%3A%2F%2Fkiwi.test%2Fota%2F"
    ));
    // A ready build is reused.
    let again = phone
        .post(&format!("/api/apps/{source}/{BUNDLE}/prepare"), json!({}))
        .await;
    assert_eq!(again.body["id"], build_id.as_str());
    assert_eq!(k.signer.signed.load(Ordering::SeqCst), 1);

    let state = app_state(&phone, &source).await;
    assert_eq!(state["action"], "install");
    assert_eq!(state["install_url"], install_url.as_str());

    // OTA: manifest, then installing.
    let ota = ota_path(&install_url);
    let manifest = k
        .raw()
        .get(format!("{}{ota}/manifest.plist", k.base))
        .header("User-Agent", INSTALLER_UA)
        .send()
        .await
        .unwrap();
    assert_eq!(manifest.status(), StatusCode::OK);
    assert_eq!(manifest.headers()[header::CONTENT_TYPE], "application/xml");
    let plist: plist::Value = plist::from_bytes(&manifest.bytes().await.unwrap()).unwrap();
    let item = plist.as_dictionary().unwrap()["items"].as_array().unwrap()[0]
        .as_dictionary()
        .unwrap()
        .clone();
    let meta = item["metadata"].as_dictionary().unwrap();
    assert_eq!(
        meta["bundle-identifier"].as_string(),
        Some(signed_id().as_str())
    );
    assert_eq!(meta["bundle-version"].as_string(), Some("1.0"));
    assert_eq!(meta["title"].as_string(), Some("Kiwi Test"));
    let assets = item["assets"].as_array().unwrap();
    assert_eq!(assets.len(), 3, "icon was cached during the build");
    let package = assets[0].as_dictionary().unwrap();
    assert_eq!(package["kind"].as_string(), Some("software-package"));
    assert_eq!(
        package["url"].as_string().unwrap(),
        format!("https://kiwi.test{ota}/app.ipa")
    );
    let icon = k
        .raw()
        .get(format!("{}{ota}/icon.png", k.base))
        .send()
        .await
        .unwrap();
    assert_eq!(icon.headers()[header::CONTENT_TYPE], "image/png");
    assert_eq!(icon.bytes().await.unwrap().as_ref(), PNG);
    assert_eq!(app_state(&phone, &source).await["action"], "installing");

    // IPA: HEAD and partial GETs do not complete the install; the full download does.
    let url = format!("{}{ota}/app.ipa", k.base);
    let head = k.raw().head(&url).send().await.unwrap();
    let len: u64 = head.headers()[header::CONTENT_LENGTH]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert_eq!(head.headers()[header::ACCEPT_RANGES], "bytes");
    assert_eq!(len, build["size"].as_u64().unwrap());
    let part = k
        .raw()
        .get(&url)
        .header("Range", "bytes=0-99")
        .send()
        .await
        .unwrap();
    assert_eq!(part.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        part.headers()[header::CONTENT_RANGE],
        format!("bytes 0-99/{len}").as_str()
    );
    assert_eq!(part.bytes().await.unwrap().len(), 100);
    let bad = k
        .raw()
        .get(&url)
        .header("Range", format!("bytes={len}-"))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), StatusCode::RANGE_NOT_SATISFIABLE);
    assert_eq!(app_state(&phone, &source).await["action"], "installing");
    let full = k
        .raw()
        .get(&url)
        .header("User-Agent", INSTALLER_UA)
        .send()
        .await
        .unwrap();
    assert_eq!(full.status(), StatusCode::OK);
    assert_eq!(
        full.headers()[header::CONTENT_TYPE],
        "application/octet-stream"
    );
    let bytes = full.bytes().await.unwrap();
    assert_eq!(bytes.len() as u64, len);
    assert_eq!(bytes.as_ref(), ipa("1.0", "1").as_slice());

    let mut state = app_state(&phone, &source).await;
    for _ in 0..100 {
        if state["action"] == "open" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        state = app_state(&phone, &source).await;
    }
    assert_eq!(state["action"], "open", "{state}");
    let scheme = kiwi_signer::url_scheme_for(&signed_id());
    assert_eq!(state["open_url"], format!("{scheme}://"));
    assert_eq!(state["installed"]["origin"], "kiwi");
    assert_eq!(state["installed"]["version"], "1.0");
    assert!(state["installed"]["expires_at"].is_string());
    let me = phone.ok("/api/me").await;
    assert_eq!(me["device"]["os_version"], "27.0.1");
    assert_eq!(me["device"]["os_version_source"], "installer");
    assert_eq!(me["device"]["model"], "iPhone19,4");
    assert_eq!(me["counts"]["installed"], 1);
    let library = phone.ok("/api/library").await;
    assert_eq!(library.as_array().unwrap().len(), 1);
    assert_eq!(library[0]["state"]["action"], "open");
    assert!(
        phone
            .ok("/api/updates")
            .await
            .as_array()
            .unwrap()
            .is_empty()
    );

    // Push subscription for update notifications.
    let sub = json!({ "endpoint": "https://web.push.apple.com/QGbYk1", "keys": push_keys() });
    let r = phone.post("/api/push/subscribe", sub).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.body);
    assert_eq!(phone.ok("/api/me").await["push"]["subscribed"], true);

    // A newer release: update; the scheduler auto-prepares it and notifies once.
    fx.set_releases(&[("2.0", "2"), ("1.0", "1")]);
    let refreshed = phone
        .post(&format!("/api/sources/{source}/refresh"), json!({}))
        .await;
    assert_eq!(refreshed.status, StatusCode::OK);
    assert!(refreshed.body["error"].is_null());
    let state = app_state(&phone, &source).await;
    assert_eq!(state["action"], "update");
    assert_eq!(state["update_reason"], "new_version");
    assert_eq!(state["target"]["version"], "2.0");
    assert!(state["install_url"].is_null());
    assert_eq!(phone.ok("/api/me").await["counts"]["updates"], 1);
    let updates = phone.ok("/api/updates").await;
    assert_eq!(updates.as_array().unwrap().len(), 1);

    let settings = Settings::default();
    scheduler::process_updates(&k.kiwi, &settings)
        .await
        .unwrap();
    assert!(
        k.push.sent.lock().unwrap().is_empty(),
        "waits for the build"
    );
    let state = app_state(&phone, &source).await;
    let update_build = state["build"]["id"]
        .as_str()
        .expect("auto-prepared")
        .to_owned();
    assert_eq!(wait_build(&phone, &update_build).await["status"], "ready");
    scheduler::process_updates(&k.kiwi, &settings)
        .await
        .unwrap();
    scheduler::process_updates(&k.kiwi, &settings)
        .await
        .unwrap();
    {
        let sent = k.push.sent.lock().unwrap();
        assert_eq!(sent.len(), 1, "{sent:?}");
        assert_eq!(sent[0].1["title"], "Kiwi Test update ready");
        assert_eq!(sent[0].1["body"], "1.0 → 2.0 · Tap to install");
        assert_eq!(sent[0].1["badge"], 1);
        assert_eq!(sent[0].1["url"], "/apps");
        assert_eq!(sent[0].1["tag"], format!("update:{}", signed_id()));
    }
    let state = app_state(&phone, &source).await;
    assert_eq!(state["action"], "update");
    assert!(state["install_url"].is_string());
    let all = phone.post("/api/updates/prepare", json!({})).await;
    assert_eq!(all.body[0]["id"], update_build.as_str());
    assert_eq!(k.signer.signed.load(Ordering::SeqCst), 2);

    // A specific older version can be prepared (it is the installed one: same build).
    let v1 = phone
        .post(
            &format!("/api/apps/{source}/{BUNDLE}/prepare"),
            json!({ "version": "1.0", "build": "1" }),
        )
        .await;
    assert_eq!(v1.body["id"], build_id.as_str());
    let none = phone
        .post(
            &format!("/api/apps/{source}/{BUNDLE}/prepare"),
            json!({ "version": "9.9" }),
        )
        .await;
    assert_eq!(none.status, StatusCode::NOT_FOUND);

    // Removing the source keeps the install as an orphan.
    let r = phone
        .send(Method::DELETE, &format!("/api/sources/{source}"), None)
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert!(phone.ok("/api/apps").await.as_array().unwrap().is_empty());
    let library = phone.ok("/api/library").await;
    assert_eq!(library[0]["orphaned"], true);
    assert_eq!(library[0]["source_name"], "Removed source");
    assert_eq!(library[0]["state"]["action"], "open");
    assert_eq!(library[0]["state"]["open_url"], format!("{scheme}://"));
    let gone = phone.get(&format!("/api/apps/{source}/{BUNDLE}")).await;
    assert_eq!(gone.status, StatusCode::NOT_FOUND);

    // Forget.
    let r = phone
        .send(
            Method::DELETE,
            &format!("/api/library/{}", signed_id()),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert!(
        phone
            .ok("/api/library")
            .await
            .as_array()
            .unwrap()
            .is_empty()
    );
    let r = phone
        .send(
            Method::DELETE,
            &format!("/api/library/{}", signed_id()),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "forgetting is idempotent");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn csrf_and_error_shape() {
    let k = TestKiwi::start().await;
    let api = k.phone();
    let url = format!("{}/api/sources", k.base);

    let r = k
        .raw()
        .post(&url)
        .json(&json!({ "url": "https://x.test" }))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    assert_eq!(r.headers()[header::CACHE_CONTROL], "no-store");
    let body: Value = r.json().await.unwrap();
    assert_eq!(body["error"]["code"], "csrf");
    assert!(
        body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("X-Kiwi")
    );

    // A cross-site form post: X-Kiwi cannot be set, and the content type is not JSON.
    let r = k
        .raw()
        .post(&url)
        .header("X-Kiwi", "1")
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body("url=https://x.test")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    let r = k.raw().delete(format!("{url}/abc")).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);

    // Empty bodies and `{}` are accepted alike.
    let r = k
        .raw()
        .post(format!("{url}/refresh"))
        .header("X-Kiwi", "1")
        .header("X-Forwarded-For", PHONE_IP)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(
        api.post("/api/sources/refresh", json!({})).await.status,
        StatusCode::OK
    );

    let r = api
        .request(Method::POST, "/api/sources")
        .header("X-Kiwi", "1")
        .header("Content-Type", "application/json")
        .body("{not json")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);
    let body: Value = r.json().await.unwrap();
    assert_eq!(body["error"]["code"], "bad_request");

    let r = api.get("/api/nope").await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(r.body["error"]["code"], "not_found");
    let r = api
        .send(Method::PATCH, "/api/health", Some(json!({})))
        .await;
    assert_eq!(r.status, StatusCode::METHOD_NOT_ALLOWED);
    assert_eq!(r.body["error"]["code"], "bad_request");
    let r = api.get("/api/builds/unknown").await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert_eq!(r.headers[header::CACHE_CONTROL], "no-store");
    assert_eq!(r.headers[header::CONTENT_TYPE], "application/json");

    // Source validation and fetch errors.
    let fx = Fixture::start().await;
    let r = api.post("/api/sources", json!({ "url": "ftp://x" })).await;
    assert_eq!(
        (r.status, r.body["error"]["code"].as_str()),
        (StatusCode::BAD_REQUEST, Some("bad_request"))
    );
    let r = api.post("/api/sources", json!({})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = api
        .post(
            "/api/sources",
            json!({ "url": format!("{}/html", fx.base) }),
        )
        .await;
    assert_eq!(
        (r.status, r.body["error"]["code"].as_str()),
        (StatusCode::BAD_GATEWAY, Some("source_fetch"))
    );
    let r = api
        .post(
            "/api/sources",
            json!({ "url": format!("{}/broken.json", fx.base) }),
        )
        .await;
    assert_eq!(
        (r.status, r.body["error"]["code"].as_str()),
        (StatusCode::UNPROCESSABLE_ENTITY, Some("source_parse"))
    );
    let r = api.post("/api/apps/none/com.x/prepare", json!({})).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);

    // Unknown OTA tokens.
    let r = k
        .raw()
        .get(format!(
            "{}/ota/AAAAAAAAAAAAAAAAAAAAAA/manifest.plist",
            k.base
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
    let r = k
        .raw()
        .get(format!("{}/ota/../app.ipa", k.base))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::NOT_FOUND);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn settings_persist_and_validate() {
    let k = TestKiwi::start().await;
    let api = k.phone();
    let s = api.ok("/api/settings").await;
    assert_eq!(s, serde_json::to_value(Settings::default()).unwrap());
    let patch = json!({ "refresh_interval_minutes": 30, "strip_watch_apps": false });
    let r = api.send(Method::PATCH, "/api/settings", Some(patch)).await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body["refresh_interval_minutes"], 30);
    assert_eq!(r.body["auto_prepare_updates"], true);
    for bad in [
        json!({ "refresh_interval_minutes": 5 }),
        json!({ "devicectl_sync_minutes": 0 }),
        json!({ "notify_updates": "no" }),
        json!({ "bogus": 1 }),
        json!([1]),
    ] {
        let r = api
            .send(Method::PATCH, "/api/settings", Some(bad.clone()))
            .await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{bad}");
        assert_eq!(r.body["error"]["code"], "bad_request");
    }

    let dir = k.stop().await;
    let k = TestKiwi::start_in(dir, FakeDevices::with_phone(), false).await;
    let s = k.phone().ok("/api/settings").await;
    assert_eq!(s["refresh_interval_minutes"], 30);
    assert_eq!(s["strip_watch_apps"], false);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cookie_devices_and_manual_os_version() {
    // Cookie devices are browsers on the Mac Kiwi runs on.
    let k = TestKiwi::start_on_mac(tempfile::tempdir().unwrap(), FakeDevices::with_phone()).await;
    let mut browser = k.browser();
    let r = browser.get("/api/me").await;
    let cookie = r.headers[header::SET_COOKIE].to_str().unwrap().to_owned();
    assert!(cookie.starts_with("kiwi_device=local:"), "{cookie}");
    assert!(cookie.contains("HttpOnly") && cookie.contains("SameSite=Lax"));
    let id = r.body["device"]["id"].as_str().unwrap().to_owned();
    assert_eq!(r.body["device"]["kind"], "browser");
    assert!(r.body["device"]["os_version"].is_null());
    assert!(
        r.body["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["code"] == "unknown_os")
    );

    browser.cookie = Some(cookie.split(';').next().unwrap().to_owned());
    let r = browser.get("/api/me").await;
    assert_eq!(r.body["device"]["id"], id.as_str());
    assert!(r.headers.get(header::SET_COOKIE).is_none());
    // Health checks do not create devices.
    let before = k.kiwi.db.devices().unwrap().len();
    k.browser().get("/api/health").await;
    assert_eq!(k.kiwi.db.devices().unwrap().len(), before);

    let r = browser
        .send(
            Method::PATCH,
            "/api/me",
            Some(json!({ "name": "  Desk  ", "os_version": "26.1" })),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.body["device"]["name"], "Desk");
    assert_eq!(r.body["device"]["os_version"], "26.1");
    assert_eq!(r.body["device"]["os_version_source"], "manual");
    let r = browser
        .send(
            Method::PATCH,
            "/api/me",
            Some(json!({ "os_version": "abc" })),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = browser
        .send(Method::PATCH, "/api/me", Some(json!({ "name": "Desk" })))
        .await;
    assert_eq!(
        r.body["device"]["os_version"], "26.1",
        "absent keeps the value"
    );
    let r = browser
        .send(
            Method::PATCH,
            "/api/me",
            Some(json!({ "os_version": null })),
        )
        .await;
    assert!(r.body["device"]["os_version"].is_null());

    // On a Mac, loopback peers with X-Forwarded-For are `tailscale serve` (other peers:
    // unit tests; other computers: tests/local.rs).
    let spoofed = k
        .raw()
        .get(format!("{}/api/me", k.base))
        .header("X-Forwarded-For", PHONE_IP)
        .send()
        .await
        .unwrap();
    let body: Value = spoofed.json().await.unwrap();
    assert_eq!(body["device"]["id"], PHONE_ID, "loopback peers are trusted");
    assert!(k.peers.lookups.lock().unwrap().is_empty());
    k.phone().get("/api/me").await;
    assert_eq!(
        k.devices.whois_calls.load(Ordering::SeqCst),
        1,
        "whois is cached"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn devices_link_and_sync() {
    let k = TestKiwi::start().await;
    let phone = k.phone();
    phone.ok("/api/me").await;
    let devices = phone.ok("/api/devices").await;
    assert_eq!(devices["devices"][0]["id"], PHONE_ID);
    let cores = devices["core_devices"].as_array().unwrap();
    assert_eq!(cores.len(), 3);
    assert!(
        cores
            .iter()
            .all(|c| c["physical"] == false && c["linked_device_id"].is_null())
    );
    let sim = cores[0]["id"].as_str().unwrap().to_owned();
    let r = phone
        .send(
            Method::PATCH,
            &format!("/api/devices/{PHONE_ID}"),
            Some(json!({ "devicectl_id": sim })),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(
        r.body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Simulators")
    );
    let r = phone
        .send(
            Method::PATCH,
            "/api/devices/ts:unknown",
            Some(json!({ "name": "x" })),
        )
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);

    // A single paired phone is linked automatically by a sync; its apps are recorded.
    let physical = fixture_devices("devicectl_list_devices_physical.json");
    let phone_core = physical
        .into_iter()
        .find(|c| c.physical && c.reachable())
        .unwrap();
    k.devices.cores.lock().unwrap().push(phone_core.clone());
    k.devices.apps.lock().unwrap().insert(
        phone_core.identifier.clone(),
        vec![DeviceApp {
            bundle_id: "com.other.app".into(),
            name: Some("Other".into()),
            version: Some("1".into()),
            build: None,
            removable: Some(true),
            path: None,
        }],
    );
    let synced = phone.post("/api/devices/sync", json!({})).await;
    assert_eq!(synced.status, StatusCode::OK, "{}", synced.body);
    let me = &synced.body["devices"][0];
    assert_eq!(me["devicectl"]["id"], phone_core.identifier.as_str());
    assert_eq!(me["devicectl"]["reachable"], true);
    assert!(me["devicectl"]["last_error"].is_null());
    assert_eq!(me["os_version_source"], "devicectl");
    let linked = synced.body["core_devices"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["id"] == phone_core.identifier.as_str())
        .unwrap()
        .clone();
    assert_eq!(linked["linked_device_id"], PHONE_ID);
    let r = phone
        .send(
            Method::PATCH,
            &format!("/api/devices/{PHONE_ID}"),
            Some(json!({ "devicectl_id": null, "name": "Phone" })),
        )
        .await;
    assert!(r.body["devicectl"].is_null());
    assert_eq!(r.body["name"], "Phone");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn devicectl_sync_verifies_and_removes_installs() {
    let fx = Fixture::start().await;
    let k = TestKiwi::start().await;
    let phone = k.phone();
    let source = add_source(&phone, &fx).await;
    let build = phone
        .post(&format!("/api/apps/{source}/{BUNDLE}/prepare"), json!({}))
        .await;
    let build = wait_build(&phone, build.body["id"].as_str().unwrap()).await;
    let ota = ota_path(build["install_url"].as_str().unwrap());
    let bytes = k
        .raw()
        .get(format!("{}{ota}/app.ipa", k.base))
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert!(!bytes.is_empty());

    let core = fixture_devices("devicectl_list_devices_physical.json")
        .into_iter()
        .find(|c| c.physical && c.reachable())
        .unwrap();
    k.devices.cores.lock().unwrap().push(core.clone());
    let app = |bundle_id: String| DeviceApp {
        bundle_id,
        name: None,
        version: Some("1.0".into()),
        build: Some("1".into()),
        removable: Some(true),
        path: None,
    };
    k.devices
        .apps
        .lock()
        .unwrap()
        .insert(core.identifier.clone(), vec![app(signed_id())]);
    phone.post("/api/devices/sync", json!({})).await;
    let state = app_state(&phone, &source).await;
    assert_eq!(state["installed"]["verified"], true);
    assert_eq!(state["action"], "open");

    // Uninstalled on the phone: the verified install is marked removed.
    k.devices
        .apps
        .lock()
        .unwrap()
        .insert(core.identifier.clone(), vec![]);
    phone.post("/api/devices/sync", json!({})).await;
    let state = app_state(&phone, &source).await;
    assert!(state["installed"].is_null());
    assert_eq!(state["action"], "install");
    assert!(
        phone
            .ok("/api/library")
            .await
            .as_array()
            .unwrap()
            .is_empty()
    );

    // An app seen under its original id (e.g. the App Store build) is listed in the library
    // but is not Kiwi's install of the source app.
    k.devices
        .apps
        .lock()
        .unwrap()
        .insert(core.identifier.clone(), vec![app(BUNDLE.into())]);
    phone.post("/api/devices/sync", json!({})).await;
    let library = phone.ok("/api/library").await;
    assert_eq!(library.as_array().unwrap().len(), 1);
    assert_eq!(library[0]["orphaned"], false);
    assert_eq!(library[0]["source_name"], "Test Source");
    assert_eq!(library[0]["state"]["action"], "open");
    assert_eq!(library[0]["state"]["installed"]["origin"], "device");
    assert_eq!(library[0]["state"]["installed"]["signed_bundle_id"], BUNDLE);
    assert!(library[0]["state"]["open_url"].is_null());
    assert_eq!(app_state(&phone, &source).await["action"], "install");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn failed_builds_retry_and_shutdown_interrupts() {
    let fx = Fixture::start().await;
    let k = TestKiwi::start().await;
    let phone = k.phone();
    let source = add_source(&phone, &fx).await;
    *k.signer.fail.lock().unwrap() = Some("codesign failed: errSecInternalComponent".into());
    let b = phone
        .post(&format!("/api/apps/{source}/{BUNDLE}/prepare"), json!({}))
        .await;
    let failed = wait_build(&phone, b.body["id"].as_str().unwrap()).await;
    assert_eq!(failed["status"], "failed");
    assert_eq!(failed["error"], "codesign failed: errSecInternalComponent");
    let state = app_state(&phone, &source).await;
    assert_eq!(state["action"], "retry");
    assert_eq!(
        state["build"]["error"],
        "codesign failed: errSecInternalComponent"
    );

    // Retry makes a new build; a shutdown while it runs marks it interrupted.
    *k.signer.fail.lock().unwrap() = None;
    k.signer.hold.store(true, Ordering::SeqCst);
    let b = phone
        .post(&format!("/api/apps/{source}/{BUNDLE}/prepare"), json!({}))
        .await;
    let id = b.body["id"].as_str().unwrap().to_owned();
    assert_ne!(id, failed["id"].as_str().unwrap());
    for _ in 0..200 {
        if phone.ok(&format!("/api/builds/{id}")).await["status"] == "signing" {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    k.kiwi.shutdown().await;
    let row = k.kiwi.db.build(&id).unwrap().unwrap();
    assert_eq!(row.error.as_deref(), Some("Interrupted by restart"));
    // The download was cached by content hash.
    let downloads: Vec<_> = std::fs::read_dir(k.kiwi.paths.downloads.clone())
        .unwrap()
        .collect();
    assert_eq!(downloads.len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn builds_left_running_fail_at_startup() {
    let dir = tempfile::tempdir().unwrap();
    let data = dir.path().join("data");
    std::fs::create_dir_all(&data).unwrap();
    {
        let db = kiwi_server::db::Db::open(&data.join("kiwi.db")).unwrap();
        let release: kiwi_source::Release = serde_json::from_value(json!({
            "version": "1.0", "download_url": "https://example.com/a.ipa"
        }))
        .unwrap();
        db.save_build(&kiwi_server::db::BuildRow {
            id: "stuck".into(),
            source_id: "s".into(),
            bundle_id: BUNDLE.into(),
            app_name: "Kiwi Test".into(),
            icon_url: None,
            version: "1.0".into(),
            build: None,
            release,
            artifact_sha256: None,
            signed_bundle_id: signed_id(),
            url_scheme: None,
            identity_sha1: None,
            profile_uuids: vec![],
            signing_key: None,
            expires_at: None,
            ipa_path: None,
            size: None,
            status: kiwi_server::db::BuildStatus::Downloading,
            step: Some("Downloading".into()),
            progress: Some(0.2),
            error: None,
            retryable: false,
            dropped_entitlements: vec![],
            created_at: chrono::Utc::now(),
            finished_at: None,
            app_version: None,
        })
        .unwrap();
        std::fs::create_dir_all(data.join("work/sign-leftover")).unwrap();
    }
    let k = TestKiwi::start_in(dir, FakeDevices::with_phone(), false).await;
    let b = k.phone().ok("/api/builds/stuck").await;
    assert_eq!(b["status"], "failed");
    assert_eq!(b["error"], "Interrupted by restart");
    assert!(b["finished_at"].is_string());
    assert_eq!(std::fs::read_dir(&k.kiwi.paths.work).unwrap().count(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn signing_push_and_static_files() {
    let k = TestKiwi::start().await;
    let api = k.phone();
    let signing = api.ok("/api/signing").await;
    assert_eq!(signing["identities"][0]["team_id"], TEAM);
    assert_eq!(signing["profiles"][0]["wildcard"], true);
    assert_eq!(signing["profiles"][0]["usable"], true);
    assert_eq!(signing["profiles"][0]["devices"], 1);
    assert_eq!(
        signing["active"]["profile_uuid"],
        "9f1c2b3a-4d5e-4f60-8a7b-0c1d2e3f4a5b"
    );
    assert!(signing["error"].is_null());
    assert_eq!(
        api.post("/api/signing/rescan", json!({})).await.body,
        signing
    );

    // Push: endpoint validation, test, unsubscribe.
    let keys = push_keys();
    let r = api
        .post(
            "/api/push/subscribe",
            json!({ "endpoint": "https://evil.test/push", "keys": keys }),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = api
        .post("/api/push/subscribe", json!({ "endpoint": "https://fcm.googleapis.com/fcm/send/x", "keys": { "p256dh": "short", "auth": "x" } }))
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let endpoint = "https://fcm.googleapis.com/fcm/send/abc";
    let r = api
        .post(
            "/api/push/subscribe",
            json!({ "endpoint": endpoint, "keys": keys }),
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let r = api.post("/api/push/test", json!({})).await;
    assert_eq!(r.body, json!({ "sent": 1 }));
    assert_eq!(k.push.sent.lock().unwrap()[0].1["title"], "Kiwi");
    let r = api
        .post("/api/push/unsubscribe", json!({ "endpoint": endpoint }))
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    assert_eq!(
        api.post("/api/push/test", json!({})).await.body,
        json!({ "sent": 0 })
    );

    // Static files and SPA fallback.
    let get = |path: &str| k.raw().get(format!("{}{path}", k.base)).send();
    let r = get("/").await.unwrap();
    assert_eq!(r.headers()[header::CACHE_CONTROL], "no-cache");
    assert!(
        r.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/html")
    );
    let r = get("/assets/index-abc123.js").await.unwrap();
    assert_eq!(
        r.headers()[header::CACHE_CONTROL],
        "public, max-age=31536000, immutable"
    );
    assert!(
        r.headers()[header::CONTENT_TYPE]
            .to_str()
            .unwrap()
            .starts_with("text/javascript")
    );
    assert_eq!(
        get("/sw.js").await.unwrap().headers()[header::CACHE_CONTROL],
        "no-cache"
    );
    assert_eq!(
        get("/manifest.webmanifest").await.unwrap().headers()[header::CONTENT_TYPE],
        "application/manifest+json"
    );
    for spa in ["/updates", "/app/abc/com.example.kiwi", "/app/a%2Fb/com.x"] {
        let r = get(spa).await.unwrap();
        assert_eq!(r.status(), StatusCode::OK, "{spa}");
        assert_eq!(
            r.text().await.unwrap(),
            "<!doctype html><title>Kiwi</title>",
            "{spa}"
        );
    }
    assert_eq!(
        get("/missing.js").await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get("/assets/missing").await.unwrap().status(),
        StatusCode::NOT_FOUND
    );
    assert_eq!(
        get("/.env").await.unwrap().text().await.unwrap(),
        "<!doctype html><title>Kiwi</title>"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_app_with_the_same_bundle_id_from_another_source_replaces_the_install() {
    let fx = Fixture::start().await;
    let k = TestKiwi::start().await;
    let phone = k.phone();
    let a = add_source(&phone, &fx).await;
    let r = phone
        .post("/api/sources", json!({ "url": fx.other_url() }))
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    let b = r.body["id"].as_str().unwrap().to_owned();
    install(&k, &phone, &a).await;
    let sub = json!({ "endpoint": "https://web.push.apple.com/QGbYk1", "keys": push_keys() });
    phone.post("/api/push/subscribe", sub).await;

    // Source B ships a newer version of a different app under the same bundle id.
    fx.set_other_releases(&[("2.0", "2"), ("1.0", "1")]);
    phone
        .post(&format!("/api/sources/{b}/refresh"), json!({}))
        .await;
    let states = || async {
        let apps = phone.ok("/api/apps").await;
        let state = |id: &str| {
            apps.as_array()
                .unwrap()
                .iter()
                .find(|x| x["source_id"] == id)
                .unwrap()["state"]
                .clone()
        };
        (state(&a), state(&b))
    };
    let (sa, sb) = states().await;
    assert_eq!(sa["action"], "open");
    assert!(sa["replaces"].is_null());
    assert_eq!(sb["action"], "get", "{sb}");
    assert!(sb["installed"].is_null() && sb["open_url"].is_null());
    assert_eq!(
        sb["replaces"],
        json!({ "name": "Kiwi Test", "source_name": "Test Source", "version": "1.0" })
    );
    let detail = phone.ok(&format!("/api/apps/{b}/{BUNDLE}")).await;
    assert_eq!(detail["state"]["replaces"]["name"], "Kiwi Test");

    // Not an update: no badge, no auto-prepared build, no notification.
    assert!(
        phone
            .ok("/api/updates")
            .await
            .as_array()
            .unwrap()
            .is_empty()
    );
    assert_eq!(phone.ok("/api/me").await["counts"]["updates"], 0);
    scheduler::process_updates(&k.kiwi, &Settings::default())
        .await
        .unwrap();
    assert!(k.kiwi.db.app_builds(&b, BUNDLE).unwrap().is_empty());
    assert_eq!(k.push.attempts(), 0);

    // Installing B replaces A's app on the phone.
    install(&k, &phone, &b).await;
    let (sa, sb) = states().await;
    assert_eq!(sb["action"], "open");
    assert_eq!(sb["installed"]["version"], "2.0");
    assert!(sb["replaces"].is_null());
    assert_eq!(
        sa["action"], "install",
        "A's ready build can be reinstalled"
    );
    assert_eq!(
        sa["replaces"],
        json!({ "name": "Kiwi Other", "source_name": "Other Source", "version": "2.0" })
    );
    let library = phone.ok("/api/library").await;
    assert_eq!(library.as_array().unwrap().len(), 1);
    assert_eq!(library[0]["source_id"], b.as_str());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn push_failures_only_count_permanent_rejections() {
    let k = TestKiwi::start().await;
    let phone = k.phone();
    let sub = json!({ "endpoint": "https://web.push.apple.com/QGbYk1", "keys": push_keys() });
    phone.post("/api/push/subscribe", sub).await;
    let payload = Payload {
        title: "Kiwi".into(),
        body: "Test".into(),
        url: "/".into(),
        badge: 0,
        tag: "t".into(),
    };
    let failures = || {
        k.kiwi
            .db
            .subscriptions(PHONE_ID)
            .unwrap()
            .first()
            .map(|s| s.failures)
    };
    let transient = SendError {
        message: "connection reset".into(),
        transient: true,
    };
    k.push.answer(Err(transient), 12);
    for status in [500, 502, 503, 408, 429] {
        let body = String::new();
        k.push.answer(Ok(Outcome::Rejected { status, body }), 3);
    }
    for _ in 0..27 {
        let sent = push::send_to_device(&k.kiwi, PHONE_ID, &payload).await;
        assert_eq!(sent.unwrap(), 0);
    }
    assert_eq!(failures(), Some(0), "transient failures are not counted");

    let rejected = Ok(Outcome::Rejected {
        status: 400,
        body: "bad request".into(),
    });
    k.push.answer(rejected, 9);
    for _ in 0..9 {
        push::send_to_device(&k.kiwi, PHONE_ID, &payload)
            .await
            .unwrap();
    }
    assert_eq!(failures(), Some(9));
    let permanent = SendError {
        message: "p256dh is not a P-256 point".into(),
        transient: false,
    };
    k.push.answer(Err(permanent), 1);
    push::send_to_device(&k.kiwi, PHONE_ID, &payload)
        .await
        .unwrap();
    assert_eq!(failures(), None, "ten permanent failures remove it");
}

/// Moves a failed build's failure (and start) back in time.
fn backdate(k: &TestKiwi, id: &str, minutes: i64) {
    let mut row = k.kiwi.db.build(id).unwrap().unwrap();
    let by = chrono::Duration::minutes(minutes);
    row.created_at -= by;
    row.finished_at = row.finished_at.map(|t| t - by);
    k.kiwi.db.save_build(&row).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn update_builds_are_retried_and_notified() {
    let fx = Fixture::start().await;
    let k = TestKiwi::start().await;
    let phone = k.phone();
    let source = add_source(&phone, &fx).await;
    install(&k, &phone, &source).await;
    let sub = json!({ "endpoint": "https://web.push.apple.com/QGbYk1", "keys": push_keys() });
    phone.post("/api/push/subscribe", sub).await;
    let mut timers = Timers::default();
    let bodies = || {
        k.push
            .sent
            .lock()
            .unwrap()
            .iter()
            .map(|(_, p)| {
                format!(
                    "{}: {}",
                    p["title"].as_str().unwrap(),
                    p["body"].as_str().unwrap()
                )
            })
            .collect::<Vec<_>>()
    };

    // 2.0 cannot be prepared automatically (signing unavailable): "available" right away.
    fx.set_releases(&[("2.0", "2"), ("1.0", "1")]);
    phone
        .post(&format!("/api/sources/{source}/refresh"), json!({}))
        .await;
    *k.signer.profile_expiry.lock().unwrap() = chrono::Utc::now() - chrono::Duration::days(1);
    k.kiwi.rescan_signing().await;
    scheduler::process_updates(&k.kiwi, &Settings::default())
        .await
        .unwrap();
    assert_eq!(bodies(), ["Kiwi Test update available: 1.0 → 2.0"]);

    // Signing is back, but the download fails: retried automatically after a minute, and no
    // notification while a retry is pending.
    *k.signer.profile_expiry.lock().unwrap() = chrono::Utc::now() + chrono::Duration::days(300);
    k.kiwi.rescan_signing().await;
    fx.fail_ipas.store(true, Ordering::SeqCst);
    scheduler::tick(&k.kiwi, &mut timers).await;
    let first = app_state(&phone, &source).await["build"]["id"]
        .as_str()
        .expect("auto-prepared")
        .to_owned();
    let failed = wait_build(&phone, &first).await;
    assert_eq!(failed["status"], "failed");
    assert!(
        failed["error"].as_str().unwrap().contains("503"),
        "{failed}"
    );
    assert!(k.kiwi.db.build(&first).unwrap().unwrap().retryable);
    scheduler::tick(&k.kiwi, &mut timers).await;
    assert_eq!(
        k.kiwi.db.app_builds(&source, BUNDLE).unwrap().len(),
        2,
        "1.0 and the failed 2.0"
    );
    assert_eq!(bodies().len(), 1, "waits for the retry");

    fx.fail_ipas.store(false, Ordering::SeqCst);
    backdate(&k, &first, 2);
    scheduler::tick(&k.kiwi, &mut timers).await;
    let retry = app_state(&phone, &source).await["build"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    assert_ne!(retry, first);
    assert_eq!(wait_build(&phone, &retry).await["status"], "ready");

    // "Ready" follows "available", once; a failed delivery is not retried on every tick.
    k.push.answer(
        Err(SendError {
            message: "timeout".into(),
            transient: true,
        }),
        1,
    );
    scheduler::tick(&k.kiwi, &mut timers).await;
    scheduler::tick(&k.kiwi, &mut timers).await;
    assert_eq!(bodies().len(), 2, "backs off after the failed delivery");
    k.kiwi
        .notices
        .delivered(&format!("update:{PHONE_ID}:{}:2.0:2:ready", signed_id()));
    scheduler::tick(&k.kiwi, &mut timers).await;
    scheduler::tick(&k.kiwi, &mut timers).await;
    assert_eq!(
        bodies(),
        [
            "Kiwi Test update available: 1.0 → 2.0",
            "Kiwi Test update ready: 1.0 → 2.0 · Tap to install",
            "Kiwi Test update ready: 1.0 → 2.0 · Tap to install"
        ]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn builds_interrupted_by_a_restart_are_requeued_but_signing_errors_are_not() {
    let fx = Fixture::start().await;
    let k = TestKiwi::start().await;
    let phone = k.phone();
    let source = add_source(&phone, &fx).await;
    k.signer.hold.store(true, Ordering::SeqCst);
    let b = phone
        .post(&format!("/api/apps/{source}/{BUNDLE}/prepare"), json!({}))
        .await;
    let id = b.body["id"].as_str().unwrap().to_owned();
    wait_until(|| {
        k.kiwi.db.build(&id).unwrap().unwrap().status == kiwi_server::db::BuildStatus::Signing
    })
    .await;
    let dir = k.stop().await;
    let k = TestKiwi::start_in(dir, FakeDevices::with_phone(), false).await;
    let phone = k.phone();
    let row = k.kiwi.db.build(&id).unwrap().unwrap();
    assert_eq!(row.error.as_deref(), Some("Interrupted by restart"));
    assert!(row.retryable);
    backdate(&k, &id, 2);
    scheduler::retry_builds(&k.kiwi).await.unwrap();
    let state = app_state(&phone, &source).await;
    let retry = state["build"]["id"].as_str().unwrap().to_owned();
    assert_ne!(retry, id);
    assert_eq!(wait_build(&phone, &retry).await["status"], "ready");
    assert!(!k.kiwi.db.build(&id).unwrap().unwrap().retryable);

    // A signing failure waits for the user.
    fx.set_releases(&[("2.0", "2"), ("1.0", "1")]);
    phone
        .post(&format!("/api/sources/{source}/refresh"), json!({}))
        .await;
    *k.signer.fail.lock().unwrap() = Some("codesign failed".into());
    let b = phone
        .post(&format!("/api/apps/{source}/{BUNDLE}/prepare"), json!({}))
        .await;
    let failed = b.body["id"].as_str().unwrap().to_owned();
    assert_eq!(wait_build(&phone, &failed).await["status"], "failed");
    assert!(!k.kiwi.db.build(&failed).unwrap().unwrap().retryable);
    backdate(&k, &failed, 30);
    scheduler::retry_builds(&k.kiwi).await.unwrap();
    assert_eq!(app_state(&phone, &source).await["action"], "retry");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn whois_errors_are_retried_then_reported() {
    let k = TestKiwi::start().await;
    k.devices.whois_errors.store(1, Ordering::SeqCst);
    let me = k.phone().ok("/api/me").await;
    assert_eq!(me["device"]["id"], PHONE_ID, "retried once");
    assert_eq!(k.devices.whois_calls.load(Ordering::SeqCst), 2);

    // Another tailnet address whose lookups keep failing: an error, not a new device.
    let mut other = k.phone();
    other.forwarded_for = Some("100.64.0.9".into());
    k.devices.whois_errors.store(2, Ordering::SeqCst);
    let r = other.get("/api/me").await;
    assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(r.body["error"]["code"], "device_unidentified");
    assert_eq!(
        r.body["error"]["message"],
        "Couldn't identify this device through Tailscale. Try again."
    );
    assert!(r.headers.get(header::SET_COOKIE).is_none());
    assert_eq!(k.kiwi.db.devices().unwrap().len(), 1);

    // The failure is remembered briefly: no new lookups for the next request.
    let calls = k.devices.whois_calls.load(Ordering::SeqCst);
    let r = other.get("/api/me").await;
    assert_eq!(r.status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(k.devices.whois_calls.load(Ordering::SeqCst), calls);

    // An address whois knows is not a peer: nobody Kiwi knows (on a Mac, a cookie device:
    // tests/local.rs).
    other.forwarded_for = Some("100.64.0.10".into());
    let r = other.get("/api/me").await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(r.body["error"]["code"], "not_owner");
    assert!(r.headers.get(header::SET_COOKIE).is_none());
    assert_eq!(k.kiwi.db.devices().unwrap().len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn devicectl_is_looked_for_again_while_missing() {
    let devices = FakeDevices::with_phone();
    devices.no_devicectl.store(true, Ordering::SeqCst);
    let k = TestKiwi::start_in(tempfile::tempdir().unwrap(), devices, false).await;
    let available =
        || async { k.phone().ok("/api/me").await["server"]["devicectl_available"].clone() };
    assert_eq!(available().await, false);
    let mut timers = Timers::default();
    scheduler::tick(&k.kiwi, &mut timers).await;
    assert_eq!(available().await, false);
    // Checked again every 10 minutes.
    k.devices.no_devicectl.store(false, Ordering::SeqCst);
    scheduler::tick(&k.kiwi, &mut timers).await;
    assert_eq!(available().await, false);
    scheduler::tick(&k.kiwi, &mut Timers::default()).await;
    assert_eq!(available().await, true);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn devicectl_sync_keeps_concurrent_device_changes() {
    let k = TestKiwi::start().await;
    let phone = k.phone();
    phone.ok("/api/me").await;
    let core = fixture_devices("devicectl_list_devices_physical.json")
        .into_iter()
        .find(|c| c.physical && c.reachable())
        .unwrap();
    k.devices.cores.lock().unwrap().push(core.clone());
    k.devices
        .apps
        .lock()
        .unwrap()
        .insert(core.identifier.clone(), vec![]);
    let synced = phone.post("/api/devices/sync", json!({})).await;
    assert_eq!(
        synced.body["devices"][0]["devicectl"]["id"],
        core.identifier.as_str()
    );

    // Rename and unlink the phone while devicectl lists its apps.
    k.devices.hold_apps.store(true, Ordering::SeqCst);
    let calls = k.devices.apps_calls.load(Ordering::SeqCst);
    let sync = tokio::spawn({
        let kiwi = k.kiwi.clone();
        async move { devices::sync(&kiwi).await }
    });
    wait_until(|| k.devices.apps_calls.load(Ordering::SeqCst) > calls).await;
    let r = phone
        .send(Method::PATCH, "/api/me", Some(json!({ "name": "Renamed" })))
        .await;
    assert_eq!(r.body["device"]["name"], "Renamed");
    let r = phone
        .send(
            Method::PATCH,
            &format!("/api/devices/{PHONE_ID}"),
            Some(json!({ "devicectl_id": null })),
        )
        .await;
    assert!(r.body["devicectl"].is_null());
    k.devices.hold_apps.store(false, Ordering::SeqCst);
    sync.await.unwrap().unwrap();

    let me = phone.ok("/api/me").await;
    assert_eq!(me["device"]["name"], "Renamed");
    assert!(me["device"]["devicectl"].is_null(), "{me}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_source_disabled_during_a_refresh_stays_disabled() {
    let fx = Fixture::start().await;
    let k = TestKiwi::start().await;
    let phone = k.phone();
    let source = add_source(&phone, &fx).await;
    fx.hold_feed.store(true, Ordering::SeqCst);
    let requests = fx.feed_requests.load(Ordering::SeqCst);
    let attempted = || k.kiwi.db.sources().unwrap()[0].last_attempt_at.unwrap();
    let added = attempted();
    let refresh = tokio::spawn({
        let phone = phone.clone();
        let path = format!("/api/sources/{source}/refresh");
        async move { phone.post(&path, json!({})).await }
    });
    wait_until(|| fx.feed_requests.load(Ordering::SeqCst) > requests).await;
    // The attempt is recorded before the fetch, so a crash while fetching is not repeated
    // as soon as the server is back.
    assert!(attempted() > added);
    let r = phone
        .send(
            Method::PATCH,
            &format!("/api/sources/{source}"),
            Some(json!({ "enabled": false })),
        )
        .await;
    assert_eq!(r.body["enabled"], false);
    fx.set_releases(&[("2.0", "2"), ("1.0", "1")]);
    fx.hold_feed.store(false, Ordering::SeqCst);

    let refreshed = refresh.await.unwrap();
    assert_eq!(refreshed.status, StatusCode::OK, "{}", refreshed.body);
    assert_eq!(refreshed.body["enabled"], false);
    assert_eq!(phone.ok("/api/sources").await[0]["enabled"], false);
    assert!(!k.kiwi.db.sources().unwrap()[0].enabled);
    assert!(phone.ok("/api/apps").await.as_array().unwrap().is_empty());
    // The fetch itself was kept.
    let app = k.kiwi.catalog.app(&source, BUNDLE).unwrap();
    assert_eq!(app.app().releases[0].version, "2.0");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn requests_for_other_hosts_are_rejected() {
    let k = TestKiwi::start().await;
    let port = k.base.rsplit(':').next().unwrap().to_owned();
    let get = |path: &str, host: Option<String>, forwarded: Option<&str>| {
        let mut r = k
            .raw()
            .get(format!("{}{path}", k.base))
            .header("X-Forwarded-For", PHONE_IP);
        if let Some(host) = host {
            r = r.header(header::HOST, host);
        }
        if let Some(forwarded) = forwarded {
            r = r.header("X-Forwarded-Host", forwarded);
        }
        r.send()
    };
    // Local curl (127.0.0.1:<port>), localhost, and tailscale serve (the public host).
    for (host, forwarded) in [
        (None, None),
        (Some(format!("localhost:{port}")), None),
        (Some("kiwi.test".to_owned()), Some("kiwi.test")),
        (Some(format!("127.0.0.1:{port}")), Some("kiwi.test")),
    ] {
        let r = get("/api/health", host.clone(), forwarded).await.unwrap();
        assert_eq!(r.status(), StatusCode::OK, "{host:?} {forwarded:?}");
    }
    // DNS rebinding: a page on another name that resolves to 127.0.0.1, with forged
    // forwarding headers.
    for (path, host, forwarded) in [
        ("/api/me", Some(format!("evil.test:{port}")), None),
        (
            "/api/me",
            Some(format!("evil.test:{port}")),
            Some("kiwi.test"),
        ),
        ("/api/me", None, Some("evil.test")),
        ("/api/me", Some("127.0.0.1:1".to_owned()), None),
        ("/", Some(format!("evil.test:{port}")), None),
        (
            "/ota/AAAAAAAAAAAAAAAAAAAAAA/app.ipa",
            Some("evil.test".to_owned()),
            None,
        ),
    ] {
        let r = get(path, host.clone(), forwarded).await.unwrap();
        assert_eq!(
            r.status(),
            StatusCode::MISDIRECTED_REQUEST,
            "{path} {host:?} {forwarded:?}"
        );
        let body: Value = r.json().await.unwrap();
        assert_eq!(body["error"]["code"], "bad_host");
    }
    assert!(k.kiwi.db.devices().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ranged_downloads_complete_only_when_they_cover_the_ipa() {
    let fx = Fixture::start().await;
    let k = TestKiwi::start().await;
    let phone = k.phone();
    let source = add_source(&phone, &fx).await;
    let b = phone
        .post(&format!("/api/apps/{source}/{BUNDLE}/prepare"), json!({}))
        .await;
    let build = wait_build(&phone, b.body["id"].as_str().unwrap()).await;
    let url = format!(
        "{}{}/app.ipa",
        k.base,
        ota_path(build["install_url"].as_str().unwrap())
    );
    let len = build["size"].as_u64().unwrap();
    let range = |spec: String| {
        let r = k.raw().get(&url).header("Range", spec);
        async move {
            let r = r.send().await.unwrap();
            assert_eq!(r.status(), StatusCode::PARTIAL_CONTENT);
            r.bytes().await.unwrap().len() as u64
        }
    };
    // Only the end of the zip (its central directory): not an install.
    let tail = len / 4;
    assert_eq!(range(format!("bytes=-{tail}")).await, tail);
    assert_eq!(range("bytes=0-99".into()).await, 100);
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert_eq!(app_state(&phone, &source).await["action"], "installing");
    assert!(k.kiwi.db.installs(PHONE_ID, true).unwrap().is_empty());
    // The rest of the file, in two overlapping ranges: complete.
    let middle = len / 2;
    assert_eq!(range(format!("bytes=50-{middle}")).await, middle - 49);
    assert!(k.kiwi.db.installs(PHONE_ID, true).unwrap().is_empty());
    assert_eq!(
        range(format!("bytes={}-{}", middle - 10, len - tail)).await,
        len - tail - middle + 11
    );
    wait_until(|| !k.kiwi.db.installs(PHONE_ID, true).unwrap().is_empty()).await;
    assert_eq!(app_state(&phone, &source).await["action"], "open");
}

/// A source listing apps (name, bundle id), each with one release.
fn multi_app_feed(base: &str, apps: &[(&str, &str)]) -> Value {
    let apps: Vec<Value> = apps
        .iter()
        .map(|(name, bundle)| {
            json!({
                "name": name, "bundleIdentifier": bundle, "developerName": "Dev",
                "versions": [{
                    "version": "1.0", "date": "2026-10-01",
                    "downloadURL": format!("{base}/ipa/app-1.0.ipa"), "size": 1000
                }]
            })
        })
        .collect();
    json!({ "name": "Grab Bag", "identifier": "grab.bag", "apps": apps })
}

fn names(apps: &Value) -> Vec<&str> {
    apps.as_array()
        .unwrap()
        .iter()
        .map(|a| a["name"].as_str().unwrap())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn apps_can_be_hidden_or_picked_from_a_source() {
    let fx = Fixture::start().await;
    let k = TestKiwi::start().await;
    let phone = k.phone();
    let main = add_source(&phone, &fx).await;
    *fx.other.lock().unwrap() = multi_app_feed(
        &fx.base,
        &[
            ("Alpha", "com.example.a"),
            ("Beta", "com.example.b"),
            ("Gamma", "com.example.c"),
        ],
    );
    let r = phone
        .post("/api/sources", json!({ "url": fx.other_url() }))
        .await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    let bag = r.body["id"].as_str().unwrap().to_owned();
    assert_eq!(
        (
            &r.body["browse"],
            &r.body["app_count"],
            &r.body["shown_count"],
            &r.body["new_count"]
        ),
        (&json!("all"), &json!(3), &json!(3), &json!(0))
    );
    let source = || async {
        let list = phone.ok("/api/sources").await;
        list.as_array()
            .unwrap()
            .iter()
            .find(|s| s["id"] == bag.as_str())
            .unwrap()
            .clone()
    };
    let patch_app = |bundle: &str, body: Value| {
        let path = format!("/api/apps/{bag}/{bundle}");
        let phone = phone.clone();
        async move { phone.send(Method::PATCH, &path, Some(body)).await }
    };

    // Hiding one app: Browse leaves it out unless hidden apps are asked for.
    let r = patch_app("com.example.b", json!({ "hidden": true })).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(
        (&r.body["hidden"], &r.body["name"]),
        (&json!(true), &json!("Beta"))
    );
    assert_eq!(
        names(&phone.ok("/api/apps").await),
        ["Alpha", "Gamma", "Kiwi Test"]
    );
    let all = phone.ok(&format!("/api/apps?source={bag}&hidden=1")).await;
    assert_eq!(names(&all), ["Alpha", "Beta", "Gamma"]);
    assert_eq!(all[1]["hidden"], true);
    assert_eq!(all[0]["hidden"], false);
    assert!(names(&phone.ok("/api/apps?q=beta").await).is_empty());
    assert_eq!(
        names(&phone.ok("/api/apps?q=beta&hidden=1").await),
        ["Beta"]
    );
    assert_eq!(source().await["shown_count"], 2);

    // Picking apps: only apps turned on are shown, the hidden one stays hidden.
    let r = phone
        .send(
            Method::PATCH,
            &format!("/api/sources/{bag}"),
            Some(json!({ "browse": "picked" })),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(
        (&r.body["browse"], &r.body["shown_count"]),
        (&json!("picked"), &json!(0))
    );
    assert_eq!(names(&phone.ok("/api/apps").await), ["Kiwi Test"]);
    patch_app("com.example.a", json!({ "hidden": false })).await;
    assert_eq!(names(&phone.ok("/api/apps").await), ["Alpha", "Kiwi Test"]);

    // Apps the source adds later are new and wait to be picked.
    *fx.other.lock().unwrap() = multi_app_feed(
        &fx.base,
        &[
            ("Alpha", "com.example.a"),
            ("Beta", "com.example.b"),
            ("Delta", "com.example.d"),
            ("Gamma", "com.example.c"),
        ],
    );
    phone
        .post(&format!("/api/sources/{bag}/refresh"), json!({}))
        .await;
    let s = source().await;
    assert_eq!(
        (&s["app_count"], &s["shown_count"], &s["new_count"]),
        (&json!(4), &json!(1), &json!(1))
    );
    let all = phone.ok(&format!("/api/apps?source={bag}&hidden=1")).await;
    let new: Vec<_> = all
        .as_array()
        .unwrap()
        .iter()
        .filter(|a| a["is_new"] == true)
        .map(|a| a["name"].as_str().unwrap())
        .collect();
    assert_eq!(new, ["Delta"]);
    let r = patch_app("com.example.d", json!({ "hidden": false })).await;
    assert_eq!(
        (&r.body["hidden"], &r.body["is_new"]),
        (&json!(false), &json!(false))
    );

    // Back to all apps: only the app hidden by hand stays hidden.
    let r = phone
        .send(
            Method::PATCH,
            &format!("/api/sources/{bag}"),
            Some(json!({ "browse": "all" })),
        )
        .await;
    assert_eq!(
        (&r.body["shown_count"], &r.body["new_count"]),
        (&json!(3), &json!(0))
    );

    // Invalid changes.
    let r = patch_app("com.example.a", json!({ "hidden": "yes" })).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = patch_app("com.example.missing", json!({ "hidden": true })).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = phone
        .send(
            Method::PATCH,
            &format!("/api/sources/{bag}"),
            Some(json!({ "browse": "some" })),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = phone
        .request(Method::PATCH, &format!("/api/apps/{bag}/com.example.a"))
        .header("Content-Type", "application/json")
        .body(r#"{"hidden":true}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN, "mutations need X-Kiwi");

    // A hidden app that is installed still shows in My Apps and gets updates.
    install(&k, &phone, &main).await;
    let r = phone
        .send(
            Method::PATCH,
            &format!("/api/apps/{main}/{BUNDLE}"),
            Some(json!({ "hidden": true })),
        )
        .await;
    assert_eq!(r.body["hidden"], true);
    fx.set_releases(&[("1.1", "2"), ("1.0", "1")]);
    phone
        .post(&format!("/api/sources/{main}/refresh"), json!({}))
        .await;
    assert!(!names(&phone.ok("/api/apps").await).contains(&"Kiwi Test"));
    assert_eq!(names(&phone.ok("/api/updates").await), ["Kiwi Test"]);
    assert_eq!(names(&phone.ok("/api/library").await), ["Kiwi Test"]);
    assert_eq!(phone.ok("/api/me").await["counts"]["updates"], 1);

    // Removing a source forgets its choices.
    let r = phone
        .send(Method::DELETE, &format!("/api/sources/{bag}"), None)
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let r = phone
        .post("/api/sources", json!({ "url": fx.other_url() }))
        .await;
    assert_eq!(
        (&r.body["browse"], &r.body["shown_count"]),
        (&json!("all"), &json!(4))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn apps_are_compared_with_their_app_store_original() {
    let fx = Fixture::start().await;
    fx.store.lock().unwrap().extend([
        (
            "us".to_owned(),
            vec![store_item(BUNDLE, "Kiwi Official: Fresh Fruit", "1.2")],
        ),
        (
            "nl".to_owned(),
            vec![
                store_item(BUNDLE, "Kiwi Official", "1.0"),
                store_item("com.example.official", "Kiwi Classic", "3.0"),
            ],
        ),
    ]);
    let k = TestKiwi::start_with_store(&fx.base).await;
    let phone = k.phone();
    let a = add_source(&phone, &fx).await;
    let r = phone
        .post("/api/sources", json!({ "url": fx.other_url() }))
        .await;
    let b = r.body["id"].as_str().unwrap().to_owned();
    let path = format!("/api/apps/{a}/{BUNDLE}");
    let detail = || phone.ok(&path);

    // Before the lookup, nothing is known.
    let d = detail().await;
    assert!(d["original"].is_null());
    assert_eq!(d["original_match"], "pending");

    // One lookup for the bundle id both sources use, in the US store by default, batched
    // with the apps Kiwi Builds offers.
    let built = kiwi_server::automation::recipes()
        .iter()
        .map(|r| r.icon_id())
        .collect::<std::collections::HashSet<_>>()
        .len();
    assert_eq!(
        kiwi_server::store::refresh(&k.kiwi).await.unwrap(),
        1 + built
    );
    assert_eq!(
        kiwi_server::store::refresh(&k.kiwi).await.unwrap(),
        0,
        "cached"
    );
    let requests = fx.store_requests.lock().unwrap().clone();
    assert_eq!(requests.len(), 1);
    assert!(
        requests[0].contains("country=us") && requests[0].contains(BUNDLE),
        "{requests:?}"
    );
    let d = detail().await;
    assert_eq!(d["original_match"], "bundle_id");
    let original = &d["original"];
    assert_eq!(
        (
            &original["bundle_id"],
            &original["name"],
            &original["short_name"],
            &original["developer"],
            &original["version"],
            &original["app_version"],
            &original["status"],
        ),
        (
            &json!(BUNDLE),
            &json!("Kiwi Official: Fresh Fruit"),
            &json!("Kiwi Official"),
            &json!("Official Dev"),
            &json!("1.2"),
            &json!("1.0"),
            &json!("behind"),
        )
    );
    assert_eq!(original["released_at"], "2026-10-20T08:00:00Z");
    assert_eq!(original["store_url"], "https://apps.apple.com/app/id1016");
    // The other source's app with the same original is another version of it.
    let others = d["other_versions"].as_array().unwrap();
    assert_eq!(others.len(), 1);
    assert_eq!(
        (
            &others[0]["source_id"],
            &others[0]["name"],
            &others[0]["version"],
            &others[0]["installed"]
        ),
        (
            &json!(b),
            &json!("Kiwi Other"),
            &json!("1.0"),
            &json!(false)
        )
    );
    let listed = phone.ok("/api/apps").await;
    assert_eq!(listed[0]["original"]["status"], "behind");

    // Another storefront: looked up again; the IPA's version is compared once built.
    let r = phone
        .send(
            Method::PATCH,
            "/api/settings",
            Some(json!({ "app_store_country": "NL" })),
        )
        .await;
    assert_eq!(r.body["app_store_country"], "nl");
    assert!(detail().await["original"].is_null());
    assert_eq!(
        kiwi_server::store::refresh(&k.kiwi).await.unwrap(),
        1 + built
    );
    install(&k, &phone, &a).await;
    let builds = k.kiwi.db.app_builds(&a, BUNDLE).unwrap();
    assert_eq!(builds[0].app_version.as_deref(), Some("1.0"));
    let original = detail().await["original"].clone();
    assert_eq!(
        (
            &original["version"],
            &original["app_version"],
            &original["status"]
        ),
        (&json!("1.0"), &json!("1.0"), &json!("same"))
    );

    // Searching the App Store, then linking an app by hand.
    let found = phone.ok("/api/store/search?term=classic").await;
    assert_eq!(found.as_array().unwrap().len(), 1);
    assert_eq!(
        (
            &found[0]["bundle_id"],
            &found[0]["short_name"],
            &found[0]["version"]
        ),
        (
            &json!("com.example.official"),
            &json!("Kiwi Classic"),
            &json!("3.0")
        )
    );
    assert!(
        fx.store_requests
            .lock()
            .unwrap()
            .last()
            .unwrap()
            .contains("term=classic")
    );
    let r = phone.get("/api/store/search?term=%20").await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let patch = |body: Value| {
        let phone = phone.clone();
        let path = format!("/api/apps/{a}/{BUNDLE}");
        async move { phone.send(Method::PATCH, &path, Some(body)).await }
    };
    let lookups = fx.store_requests.lock().unwrap().len();
    let r = patch(json!({ "original": { "bundle_id": "com.example.official" } })).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(
        fx.store_requests.lock().unwrap().len(),
        lookups,
        "cached by the search"
    );
    assert_eq!(r.body["original_match"], "linked");
    assert_eq!(
        (
            &r.body["original"]["short_name"],
            &r.body["original"]["status"]
        ),
        (&json!("Kiwi Classic"), &json!("behind"))
    );
    assert!(r.body["other_versions"].as_array().unwrap().is_empty());

    // Unknown apps are looked up and refused.
    let r = patch(json!({ "original": { "bundle_id": "com.example.nowhere" } })).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(
        r.body["error"]["message"],
        "No app in the App Store (NL) has the bundle ID com.example.nowhere."
    );
    assert_eq!(fx.store_requests.lock().unwrap().len(), lookups + 1);

    // Not based on an App Store app, then automatic again.
    let r = patch(json!({ "original": { "bundle_id": null } })).await;
    assert_eq!(
        (&r.body["original_match"], &r.body["original"]),
        (&json!("none"), &Value::Null)
    );
    let r = patch(json!({ "original": null })).await;
    assert_eq!(r.body["original_match"], "bundle_id");
    assert_eq!(r.body["original"]["short_name"], "Kiwi Official");

    // A failed lookup is retried later, not on every tick.
    fx.fail_store.store(true, Ordering::SeqCst);
    phone
        .send(
            Method::PATCH,
            "/api/settings",
            Some(json!({ "app_store_country": "de" })),
        )
        .await;
    let err = kiwi_server::store::refresh(&k.kiwi).await.unwrap_err();
    assert!(err.contains("503"), "{err}");
    let failed = fx.store_requests.lock().unwrap().len();
    assert_eq!(kiwi_server::store::refresh(&k.kiwi).await.unwrap(), 0);
    assert_eq!(fx.store_requests.lock().unwrap().len(), failed);
    let r = phone.get("/api/store/search?term=kiwi").await;
    assert_eq!(r.status, StatusCode::BAD_GATEWAY);
    assert_eq!(r.body["error"]["code"], "app_store");
}
