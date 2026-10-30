//! The family: members, enrollment, approval, registration with Apple and app access.

mod common;

use common::*;
use reqwest::{Method, StatusCode};
use serde_json::{Value, json};

/// A Kiwi with the fixture source added by the owner (whose phone has notifications on)
/// and Noah's iPhone on the tailnet.
async fn family(fx: &Fixture, appstore: Option<&FakeAppStoreConnect>) -> (TestKiwi, String) {
    let devices = FakeDevices::with_phone();
    devices.add_member(NOAH_IP, "nNOAH1CNTRL", NOAH_LOGIN, "Noah Morgan");
    let k = TestKiwi::start_full(
        tempfile::tempdir().unwrap(),
        devices,
        false,
        "http://127.0.0.1:9",
        appstore.map_or("http://127.0.0.1:9", |a| a.base.as_str()),
    )
    .await;
    let phone = k.phone();
    let source = add_source(&phone, fx).await;
    let r = phone
        .post(
            "/api/push/subscribe",
            json!({ "endpoint": "https://web.push.apple.com/owner", "keys": push_keys() }),
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    (k, source)
}

fn pushes(k: &TestKiwi, endpoint: &str) -> Vec<Value> {
    k.push
        .sent
        .lock()
        .unwrap()
        .iter()
        .filter(|(e, _)| e == endpoint)
        .map(|(_, v)| v.clone())
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn members_register_and_see_only_their_apps() {
    let fx = Fixture::start().await;
    let (k, source) = family(&fx, None).await;
    let phone = k.phone();
    let noah = k.device(NOAH_IP);

    // The owner is the Mac's Tailscale user.
    let me = phone.ok("/api/me").await;
    assert_eq!(me["access"]["role"], "owner");
    assert_eq!(me["access"]["owner_name"], "Alex");
    assert_eq!(me["registration"]["ready"], true);

    // Noah is a member: nothing to see or do before registering.
    let me = noah.ok("/api/me").await;
    assert_eq!(me["access"]["role"], "member");
    assert_eq!(me["access"]["person"]["id"], NOAH_LOGIN);
    assert_eq!(me["access"]["person"]["name"], "Noah Morgan");
    assert_eq!(me["access"]["owner_name"], "Alex");
    assert_eq!(me["registration"]["status"], "none");
    assert_eq!(me["registration"]["ready"], false);
    assert_eq!(me["counts"]["sources"], 0);
    assert!(noah.ok("/api/apps").await.as_array().unwrap().is_empty());
    for path in [
        "/api/sources",
        "/api/settings",
        "/api/signing",
        "/api/family",
        "/api/devices",
    ] {
        let r = noah.get(path).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{path}");
        assert_eq!(r.body["error"]["code"], "forbidden");
        assert_eq!(r.body["error"]["message"], "Only Alex can do this.");
    }
    let prepared = noah
        .post(&format!("/api/apps/{source}/{BUNDLE}/prepare"), json!({}))
        .await;
    assert_eq!(prepared.status, StatusCode::FORBIDDEN);
    assert_eq!(prepared.body["error"]["code"], "not_registered");
    let added = noah
        .post("/api/sources", json!({ "url": fx.feed_url() }))
        .await;
    assert_eq!(added.status, StatusCode::FORBIDDEN);

    // Enrollment sends the UDID; Kiwi names the iPhone and asks the owner.
    assert_eq!(
        enroll(&k, &noah, NOAH_UDID).await,
        "https://kiwi.test/?enrolled=1"
    );
    let me = noah.ok("/api/me").await;
    assert_eq!(me["registration"]["status"], "pending");
    assert_eq!(me["registration"]["product"], "iPhone15,4");
    assert_eq!(me["registration"]["udid"], NOAH_UDID);
    assert!(me["registration"]["error"].is_null());
    assert_eq!(me["device"]["name"], "Noah's iPhone");
    let asked = pushes(&k, "https://web.push.apple.com/owner");
    assert_eq!(asked.len(), 1, "{asked:?}");
    assert_eq!(asked[0]["title"], "Register Noah's iPhone?");
    assert_eq!(
        asked[0]["url"],
        format!("/settings/family/devices/{NOAH_ID}")
    );
    // A used or unknown token is refused.
    let r = k
        .raw()
        .post(format!("{}/enroll/nope", k.base))
        .body(enrollment_body("nope", NOAH_UDID, "iPhone15,4"))
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::BAD_REQUEST);

    let family = phone.ok("/api/family").await;
    assert_eq!(family["pending"], 1);
    let people = family["people"].as_array().unwrap();
    assert_eq!(people.len(), 2);
    assert_eq!(
        (people[0]["id"].as_str(), people[0]["role"].as_str()),
        (Some("alex@example.com"), Some("owner"))
    );
    assert_eq!(people[0]["name"], "Alex Morgan");
    assert_eq!(people[0]["devices"][0]["id"], PHONE_ID);
    assert_eq!(people[1]["id"], NOAH_LOGIN);
    assert_eq!(people[1]["app_count"], 0);
    assert_eq!(people[1]["devices"][0]["registration"]["status"], "pending");
    assert_eq!(family["apple"]["connected"], false);
    assert_eq!(family["apple"]["profile"]["devices"], 1);

    // The owner picks Noah's apps and approves.
    let person = format!("/api/family/people/{NOAH_LOGIN}");
    let chosen = phone
        .send(
            Method::PATCH,
            &format!("{person}/apps"),
            Some(
                json!({ "apps": [{ "source_id": source, "bundle_id": BUNDLE, "allowed": true }] }),
            ),
        )
        .await;
    assert_eq!(chosen.status, StatusCode::OK, "{}", chosen.body);
    assert_eq!(chosen.body["app_count"], 1);
    assert_eq!(chosen.body["apps"][0]["allowed"], true);
    assert_eq!(chosen.body["apps"][0]["explicit"], true);
    let approved = phone
        .post(&format!("/api/family/devices/{NOAH_ID}/approve"), json!({}))
        .await;
    assert_eq!(approved.status, StatusCode::OK, "{}", approved.body);
    assert_eq!(approved.body["registration"]["status"], "approved");
    assert_eq!(approved.body["registration"]["in_profile"], false);

    // Without an App Store Connect key the owner adds the iPhone to a profile by hand;
    // until then Noah still sees nothing.
    let me = noah.ok("/api/me").await;
    assert_eq!(me["registration"]["ready"], false);
    assert!(noah.ok("/api/apps").await.as_array().unwrap().is_empty());
    let r = noah
        .post(
            "/api/push/subscribe",
            json!({ "endpoint": "https://web.push.apple.com/noah", "keys": push_keys() }),
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let profiles = k.dir.path().join("data/profiles");
    std::fs::write(
        profiles.join("manual.mobileprovision"),
        mobileprovision(
            "11111111-0000-4000-8000-000000000001",
            "Family",
            "*",
            &[ALEX_UDID.into(), NOAH_UDID.into()],
            chrono::Utc::now() + chrono::Duration::days(200),
        ),
    )
    .unwrap();
    let rescan = phone.post("/api/signing/rescan", json!({})).await;
    assert_eq!(rescan.status, StatusCode::OK);
    let me = noah.ok("/api/me").await;
    assert_eq!(me["registration"]["in_profile"], true);
    assert_eq!(me["registration"]["ready"], true);
    kiwi_server::family::notify_ready(&k.kiwi).await.unwrap();
    let ready = pushes(&k, "https://web.push.apple.com/noah");
    assert_eq!(ready.len(), 1);
    assert_eq!(
        ready[0]["body"],
        "This iPhone is ready. Open Kiwi to install apps."
    );

    // Noah sees the app the owner allowed (never hidden by the owner's Browse choices)
    // and installs it with a build whose profile includes the iPhone.
    let hidden = phone
        .send(
            Method::PATCH,
            &format!("/api/apps/{source}/{BUNDLE}"),
            Some(json!({ "hidden": true })),
        )
        .await;
    assert_eq!(hidden.status, StatusCode::OK);
    let apps = noah.ok("/api/apps").await;
    assert_eq!(apps.as_array().unwrap().len(), 1);
    assert_eq!(apps[0]["hidden"], false);
    assert!(noah.ok(&format!("/api/apps/{source}/{BUNDLE}")).await["access"].is_null());
    let build = install(&k, &noah, &source).await;
    let row = k
        .kiwi
        .db
        .build(build["id"].as_str().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(row.profile_uuids, ["11111111-0000-4000-8000-000000000001"]);
    assert_eq!(
        row.devices,
        Some(vec![ALEX_UDID.to_owned(), NOAH_UDID.to_owned()])
    );
    let request = k.signer.requests.lock().unwrap().last().cloned().unwrap();
    assert_eq!(request.devices, [NOAH_UDID]);

    // The owner sees who can install it; turning it off hides it from Noah, while the
    // installed copy stays in Noah's library without updates.
    let detail = phone.ok(&format!("/api/apps/{source}/{BUNDLE}")).await;
    let access = detail["access"].as_array().unwrap();
    assert_eq!(access.len(), 2);
    assert_eq!(
        (access[0]["role"].as_str(), access[0]["allowed"].as_bool()),
        (Some("owner"), Some(true))
    );
    assert_eq!(access[1]["person_id"], NOAH_LOGIN);
    assert_eq!(access[1]["installed_version"], "1.0");
    assert_eq!(access[1]["pending"], false);
    phone
        .send(
            Method::PATCH,
            &format!("{person}/apps"),
            Some(
                json!({ "apps": [{ "source_id": source, "bundle_id": BUNDLE, "allowed": false }] }),
            ),
        )
        .await;
    assert!(noah.ok("/api/apps").await.as_array().unwrap().is_empty());
    let r = noah.get(&format!("/api/apps/{source}/{BUNDLE}")).await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let library = noah.ok("/api/library").await;
    assert_eq!(library[0]["orphaned"], true);
    assert_eq!(library[0]["state"]["action"], "open");
    let r = noah
        .get(&format!("/api/builds/{}", build["id"].as_str().unwrap()))
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);

    // "Apps You Add Later": only apps first seen after it was turned on.
    let on = phone
        .send(
            Method::PATCH,
            &person,
            Some(json!({ "new_apps": true, "name": "Noah" })),
        )
        .await;
    assert_eq!(on.body["new_apps"], true);
    assert_eq!(on.body["name"], "Noah");
    assert_eq!(on.body["apps"][0]["allowed"], false);
    let other = phone
        .post("/api/sources", json!({ "url": fx.other_url() }))
        .await;
    assert_eq!(other.status, StatusCode::CREATED);
    let apps = noah.ok("/api/apps").await;
    assert_eq!(apps.as_array().unwrap().len(), 1);
    assert_eq!(apps[0]["source_name"], "Other Source");

    // Removing Noah forgets the registration: the iPhone has to register again.
    let r = phone.send(Method::DELETE, &person, None).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);
    let me = noah.ok("/api/me").await;
    assert_eq!(me["registration"]["status"], "none");
    assert!(noah.ok("/api/apps").await.as_array().unwrap().is_empty());
    let owner = phone
        .send(Method::DELETE, "/api/family/people/alex@example.com", None)
        .await;
    assert_eq!(owner.status, StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn declined_devices_stay_out() {
    let fx = Fixture::start().await;
    let (k, _source) = family(&fx, None).await;
    let phone = k.phone();
    let noah = k.device(NOAH_IP);
    enroll(&k, &noah, NOAH_UDID).await;
    let declined = phone
        .post(&format!("/api/family/devices/{NOAH_ID}/decline"), json!({}))
        .await;
    assert_eq!(declined.body["registration"]["status"], "declined");
    assert_eq!(noah.ok("/api/me").await["registration"]["ready"], false);
    assert_eq!(phone.ok("/api/family").await["pending"], 0);
    // Enrolling again asks again.
    enroll(&k, &noah, NOAH_UDID).await;
    assert_eq!(
        noah.ok("/api/me").await["registration"]["status"],
        "pending"
    );
    // Unknown devices and the owner's own enrollment.
    let r = phone
        .post("/api/family/devices/ts:nope/approve", json!({}))
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    enroll(&k, &phone, ALEX_UDID).await;
    let me = phone.ok("/api/me").await;
    assert_eq!(me["registration"]["status"], "approved");
    assert_eq!(me["registration"]["in_profile"], true);
    // Browsers on the Mac cannot enroll.
    let r = k.browser().post("/api/me/enroll", json!({})).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn app_store_connect_registers_devices_and_makes_profiles() {
    let fx = Fixture::start().await;
    let asc = FakeAppStoreConnect::start().await;
    let (k, source) = family(&fx, Some(&asc)).await;
    let phone = k.phone();
    let noah = k.device(NOAH_IP);

    // A key App Store Connect refuses is not saved.
    let key = json!({
        "issuer_id": "57246542-96fe-1a63-e053-0824d011072a",
        "key_id": "2x9r4hxf34",
        "private_key": p8(),
    });
    asc.reject_key
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let r = phone
        .send(Method::PUT, "/api/family/apple", Some(key.clone()))
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.body);
    assert!(
        r.body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("did not accept")
    );
    let r = phone
        .send(
            Method::PUT,
            "/api/family/apple",
            Some(json!({ "issuer_id": "x", "key_id": "y", "private_key": "z" })),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(phone.ok("/api/family/apple").await["connected"], false);
    asc.reject_key
        .store(false, std::sync::atomic::Ordering::SeqCst);
    let r = phone
        .send(Method::PUT, "/api/family/apple", Some(key))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.body["connected"], true);
    assert_eq!(r.body["key_id"], "2X9R4HXF34");
    assert_eq!(r.body["devices"], 1);
    assert_eq!(r.body["device_limit"], 100);
    let saved = k.dir.path().join("data/appstoreconnect.json");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(&saved).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }
    assert!(!r.body.to_string().contains("PRIVATE KEY"));

    // Noah enrolls; approving registers the iPhone and makes a profile with every device.
    enroll(&k, &noah, NOAH_UDID).await;
    let chosen = phone
        .send(
            Method::PATCH,
            &format!("/api/family/people/{NOAH_LOGIN}/apps"),
            Some(
                json!({ "apps": [{ "source_id": source, "bundle_id": BUNDLE, "allowed": true }] }),
            ),
        )
        .await;
    assert_eq!(chosen.status, StatusCode::OK, "{}", chosen.body);
    phone
        .post(&format!("/api/family/devices/{NOAH_ID}/approve"), json!({}))
        .await;
    let mut ready = false;
    for _ in 0..200 {
        if noah.ok("/api/me").await["registration"]["ready"] == true {
            ready = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert!(ready, "{:?}", asc.requests.lock().unwrap());
    let devices = asc.devices.lock().unwrap().clone();
    assert_eq!(devices.len(), 2);
    assert_eq!(devices[1].1, NOAH_UDID);
    let created = asc.profiles.lock().unwrap().clone();
    assert_eq!(created.len(), 1);
    let rel = &created[0]["data"]["relationships"];
    assert_eq!(rel["bundleId"]["data"]["id"], "BWILD");
    assert_eq!(rel["certificates"]["data"][0]["id"], "CDEV");
    assert_eq!(rel["devices"]["data"].as_array().unwrap().len(), 2);
    let apple = phone.ok("/api/family/apple").await;
    assert_eq!(apple["profile"]["kiwi"], true, "{apple}");
    assert_eq!(apple["profile"]["devices"], 2);
    assert!(
        apple["profile"]["name"]
            .as_str()
            .unwrap()
            .starts_with("Kiwi * ")
    );
    assert!(apple["error"].is_null());
    let device = phone.ok(&format!("/api/family/devices/{NOAH_ID}")).await;
    assert_eq!(device["registration"]["apple_registered"], true);

    // Syncing again changes nothing: the profile already includes everyone.
    let synced = phone.post("/api/family/apple/sync", json!({})).await;
    assert_eq!(synced.status, StatusCode::OK);
    assert_eq!(asc.profiles.lock().unwrap().len(), 1);
    assert_eq!(asc.devices.lock().unwrap().len(), 2);

    // Builds for everyone now use Kiwi's profile.
    let build = install(&k, &noah, &source).await;
    let row = k
        .kiwi
        .db
        .build(build["id"].as_str().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(row.devices.as_ref().unwrap().len(), 2);

    // Forgetting the key.
    let r = phone.send(Method::DELETE, "/api/family/apple", None).await;
    assert_eq!(r.body["connected"], false);
    assert!(!saved.exists());
}
