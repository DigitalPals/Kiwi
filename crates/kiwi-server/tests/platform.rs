//! Device platforms, the first-run setup, Kiwi's Android app and its User-Agent
//! (api.md "Me", "Android devices", "Kiwi's Android app").

mod common;

use common::*;
use reqwest::{Method, StatusCode, header};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};

const ANDROID_IP: &str = "100.64.0.9";
const DESKTOP_IP: &str = "100.64.0.11";
const NATIVE_UA: &str = "Kiwi-Android/1.2.0 (12; Android 15; SDK 35; Google Pixel 9a)";
const MACOS: bool = cfg!(target_os = "macos");

fn features(me: &Value) -> Vec<&str> {
    me["features"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| f.as_str().unwrap())
        .collect()
}

fn with_devicectl(mut list: Vec<&'static str>) -> Vec<&'static str> {
    if MACOS {
        list.push("devicectl");
    }
    list
}

async fn patch_me(api: &Api, body: Value) -> Reply {
    api.send(Method::PATCH, "/api/me", Some(body)).await
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn devices_have_a_platform_and_a_first_run_setup() {
    let k = TestKiwi::start().await;
    k.devices.add_android(ANDROID_IP);

    // An iPhone: Tailscale and Safari say iOS.
    let phone = k.phone();
    let me = phone.ok("/api/me").await;
    let device = &me["device"];
    assert_eq!(device["platform"], "ios");
    assert_eq!(device["platform_source"], "detected");
    assert_eq!(device["detected_platform"], "ios");
    assert_eq!(device["client"], "web");
    assert_eq!(me["onboarded"], false);
    assert_eq!(
        me["host"],
        json!({
            "os": if MACOS { "macos" } else { "linux" },
            "ios_signing": if MACOS { "keychain" } else { "files" },
            "devicectl": true,
            "android_signer_sha256": null,
        })
    );
    assert!(me["android_app"].is_null());
    assert_eq!(
        features(&me),
        with_devicectl(vec!["ios_apps", "home_screen_app", "web_push"])
    );

    // An Android phone in Chrome.
    let android = k.android(ANDROID_IP);
    let me = android.ok("/api/me").await;
    assert_eq!(me["device"]["platform"], "android");
    assert_eq!(me["device"]["platform_source"], "detected");
    assert_eq!(me["device"]["model"], "Android");
    assert_eq!(features(&me), ["android_apps", "web_push"]);

    // A desktop browser tells nothing, nor does Tailscale for a desktop computer: iOS is
    // assumed, and either platform can be chosen.
    k.devices.add_computer(DESKTOP_IP, "linux");
    let desktop = Api {
        user_agent: Some(
            "Mozilla/5.0 (X11; Linux x86_64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36".into(),
        ),
        ..k.device(DESKTOP_IP)
    };
    let r = desktop.get("/api/me").await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let device = &r.body["device"];
    assert_eq!(device["platform"], "ios");
    assert_eq!(device["platform_source"], "assumed");
    assert!(device["detected_platform"].is_null());
    let me = patch_me(&desktop, json!({ "platform": "android" })).await;
    assert_eq!(me.status, StatusCode::OK, "{}", me.body);
    assert_eq!(me.body["device"]["platform"], "android");
    assert_eq!(me.body["device"]["platform_source"], "chosen");
    assert!(me.body["device"]["detected_platform"].is_null());
    assert_eq!(features(&me.body), ["android_apps", "web_push"]);
    assert_eq!(
        me.body["warnings"],
        json!([]),
        "no iOS warnings for an Android device"
    );
    assert_eq!(desktop.ok("/api/me").await["device"]["platform"], "android");
    let me = patch_me(&desktop, json!({ "platform": "ios" })).await;
    assert_eq!(me.body["device"]["platform_source"], "chosen");
    assert!(
        me.body["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w["code"] == "unknown_os")
    );
    let me = patch_me(&desktop, json!({ "platform": null })).await;
    assert_eq!(me.body["device"]["platform"], "ios");
    assert_eq!(me.body["device"]["platform_source"], "assumed");

    // The owner's choice wins over detection, also for a phone.
    let me = patch_me(&phone, json!({ "platform": "android" })).await;
    assert_eq!(me.body["device"]["platform"], "android");
    assert_eq!(me.body["device"]["detected_platform"], "ios");
    assert_eq!(features(&me.body), ["android_apps", "web_push"]);
    let me = patch_me(&phone, json!({ "platform": null })).await;
    assert_eq!(me.body["device"]["platform"], "ios");
    assert_eq!(me.body["device"]["platform_source"], "detected");

    for bad in [
        json!({ "platform": "windows" }),
        json!({ "platform": 1 }),
        json!({ "onboarded": "yes" }),
        json!({ "theme": "dark" }),
    ] {
        let r = patch_me(&phone, bad.clone()).await;
        assert_eq!(r.status, StatusCode::BAD_REQUEST, "{bad}");
        assert_eq!(r.body["error"]["code"], "bad_request");
    }

    // The first-run setup: done keeps its first time, "Show Setup Again" clears it.
    let me = patch_me(&phone, json!({ "onboarded": true })).await;
    assert_eq!(me.body["onboarded"], true);
    let first = k
        .kiwi
        .db
        .device(PHONE_ID)
        .unwrap()
        .unwrap()
        .onboarded_at
        .unwrap();
    let me = patch_me(&phone, json!({ "onboarded": true, "name": "Mine" })).await;
    assert_eq!(
        (
            me.body["onboarded"].clone(),
            me.body["device"]["name"].clone()
        ),
        (json!(true), json!("Mine"))
    );
    let device = k.kiwi.db.device(PHONE_ID).unwrap().unwrap();
    assert_eq!(device.onboarded_at, Some(first));
    assert_eq!(
        patch_me(&phone, json!({})).await.body["onboarded"],
        true,
        "absent keeps it"
    );
    let me = patch_me(&phone, json!({ "onboarded": false })).await;
    assert_eq!(me.body["onboarded"], false);

    // GET /api/devices shows every device's platform; the client is this request's.
    let list = phone.ok("/api/devices").await;
    let find = |id: &str| -> Value {
        list["devices"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["id"] == id)
            .cloned()
            .unwrap()
    };
    assert_eq!(find(PHONE_ID)["platform"], "ios");
    assert_eq!(find("ts:nANDROID1CNTRL")["platform"], "android");
    assert_eq!(find("ts:nANDROID1CNTRL")["client"], "web");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kiwis_android_app_identifies_itself() {
    let k = TestKiwi::start().await;
    k.devices.add_android(ANDROID_IP);
    // Chrome first: the same phone, one device.
    let chrome = k.android(ANDROID_IP);
    chrome.ok("/api/me").await;
    let app = Api {
        user_agent: Some(NATIVE_UA.into()),
        ..k.android(ANDROID_IP)
    };
    let me = app.ok("/api/me").await;
    let device = &me["device"];
    assert_eq!(device["id"], "ts:nANDROID1CNTRL");
    assert_eq!(device["client"], "android_app");
    assert_eq!(device["platform"], "android");
    assert_eq!(device["os_version"], "15");
    assert_eq!(device["os_version_source"], "android_app");
    assert_eq!(features(&me), ["android_apps"]);
    assert_eq!(chrome.ok("/api/me").await["device"]["client"], "web");
    assert_eq!(k.kiwi.db.devices().unwrap().len(), 1);

    // The iPhone can't send packages; a native-app-only endpoint for Android devices.
    let r = k
        .phone()
        .send(
            Method::PUT,
            "/api/me/packages",
            Some(json!({ "packages": [] })),
        )
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    let r = app
        .send(
            Method::PUT,
            "/api/me/packages",
            Some(json!({ "packages": [] })),
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.body);
    let r = app
        .send(Method::PUT, "/api/me/packages", Some(json!({ "apps": [] })))
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    // Without X-Kiwi it is refused like every mutation.
    let r = app
        .request(Method::PUT, "/api/me/packages")
        .header("Content-Type", "application/json")
        .body(r#"{"packages":[]}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::FORBIDDEN);
    // A phone's full package list fits.
    let packages: Vec<Value> = (0..1500)
        .map(|i| {
            json!({
                "package": format!("com.example.app{i}"), "version_name": "1.0.0",
                "version_code": i, "signer_sha256": ["ab".repeat(32)],
                "installer": "com.android.vending", "first_install_time": 0,
            })
        })
        .collect();
    let r = app
        .send(
            Method::PUT,
            "/api/me/packages",
            Some(json!({ "packages": packages })),
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.body);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn browse_follows_the_chosen_platform() {
    let fx = Fixture::start().await;
    let k = TestKiwi::start().await;
    let phone = k.phone();
    let source = add_source(&phone, &fx).await;
    let names = |apps: Value| -> Vec<String> {
        apps.as_array()
            .unwrap()
            .iter()
            .map(|a| a["bundle_id"].as_str().unwrap().to_owned())
            .collect()
    };
    assert_eq!(names(phone.ok("/api/apps").await), [BUNDLE]);
    patch_me(&phone, json!({ "platform": "android" })).await;
    assert!(names(phone.ok("/api/apps").await).is_empty());
    let app = phone.ok(&format!("/api/apps/{source}/{BUNDLE}")).await;
    assert_eq!(app["unavailable"], "Made for iPhone and iPad");
    assert_eq!(app["state"]["action"], "unavailable");
    assert_eq!(app["state"]["foreign_install"], false);
    let r = phone
        .post(&format!("/api/apps/{source}/{BUNDLE}/prepare"), json!({}))
        .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    patch_me(&phone, json!({ "platform": "ios" })).await;
    assert_eq!(names(phone.ok("/api/apps").await), [BUNDLE]);

    // An iOS device whose version is unknown is offered every iOS release.
    k.devices
        .add_peer("100.64.0.12", "nIPAD1CNTRL", "alex@example.com", "iPad");
    let ipad = Api {
        user_agent: None,
        ..k.device("100.64.0.12")
    };
    let app = ipad.ok(&format!("/api/apps/{source}/{BUNDLE}")).await;
    assert_eq!(app["state"]["action"], "get", "{app}");
    assert!(app["unavailable"].is_null());
}

fn write_android_app(k: &TestKiwi, bytes: &[u8], version: &str) -> String {
    let dir = k.dir.path().join("data/android-app");
    std::fs::create_dir_all(&dir).unwrap();
    let sha = hex::encode(Sha256::digest(bytes));
    std::fs::write(dir.join("kiwi.apk"), bytes).unwrap();
    std::fs::write(
        dir.join("kiwi.json"),
        json!({
            "package": "io.github.digitalpals.kiwi", "version_name": version,
            "version_code": 12, "sha256": sha, "size": bytes.len(),
        })
        .to_string(),
    )
    .unwrap();
    sha
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn kiwis_android_app_is_served_to_the_owners_devices() {
    let k = TestKiwi::start().await;
    k.devices.add_android(ANDROID_IP);
    k.devices
        .add_peer("100.64.0.20", "nOTHER1CNTRL", "sam@example.com", "Sam");
    let android = k.android(ANDROID_IP);
    let phone = k.phone();
    assert!(android.ok("/api/me").await["android_app"].is_null());
    let r = android.get("/android/kiwi.apk").await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);

    let bytes: Vec<u8> = b"PK\x03\x04"
        .iter()
        .copied()
        .chain((0..100_000).map(|i| (i % 251) as u8))
        .collect();
    write_android_app(&k, &bytes, "1.2.0");
    // A web file with the same name never shadows the app.
    let web = k.dir.path().join("web/android");
    std::fs::create_dir_all(&web).unwrap();
    std::fs::write(web.join("kiwi.apk"), b"stale").unwrap();

    let me = android.ok("/api/me").await;
    assert_eq!(
        me["android_app"],
        json!({
            "package": "io.github.digitalpals.kiwi", "version_name": "1.2.0",
            "version_code": 12, "size": bytes.len(),
            "install_url": "https://kiwi.test/android/kiwi.apk",
        })
    );
    assert_eq!(
        features(&me),
        ["android_apps", "web_push", "android_app_offer"]
    );
    let me = phone.ok("/api/me").await;
    assert!(me["android_app"].is_object());
    assert!(!features(&me).contains(&"android_app_offer"));
    let native = Api {
        user_agent: Some(NATIVE_UA.into()),
        ..k.android(ANDROID_IP)
    };
    assert!(!features(&native.ok("/api/me").await).contains(&"android_app_offer"));

    let response = android
        .request(Method::GET, "/android/kiwi.apk")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let headers = response.headers().clone();
    assert_eq!(
        headers[header::CONTENT_TYPE],
        "application/vnd.android.package-archive"
    );
    assert_eq!(
        headers[header::CONTENT_DISPOSITION],
        "attachment; filename=\"Kiwi-1.2.0.apk\""
    );
    assert_eq!(headers[header::ACCEPT_RANGES], "bytes");
    assert_eq!(&response.bytes().await.unwrap()[..], &bytes[..]);
    let partial = android
        .request(Method::GET, "/android/kiwi.apk")
        .header("Range", "bytes=2-5")
        .send()
        .await
        .unwrap();
    assert_eq!(partial.status(), StatusCode::PARTIAL_CONTENT);
    assert_eq!(
        partial.headers()[header::CONTENT_RANGE],
        format!("bytes 2-5/{}", bytes.len())
    );
    assert_eq!(&partial.bytes().await.unwrap()[..], &bytes[2..=5]);
    let head = android
        .request(Method::HEAD, "/android/kiwi.apk")
        .send()
        .await
        .unwrap();
    assert_eq!(head.status(), StatusCode::OK);
    assert_eq!(
        head.headers()[header::CONTENT_LENGTH],
        bytes.len().to_string()
    );
    // Only the owner's devices get it, like every API.
    let r = k.device("100.64.0.20").get("/android/kiwi.apk").await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(r.body["error"]["code"], "not_owner");
    // Other paths there are not the web app.
    let r = android.get("/android/other.apk").await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);

    // A file that doesn't match its description is not offered.
    let dir = k.dir.path().join("data/android-app");
    std::fs::write(dir.join("kiwi.apk"), b"PK tampered").unwrap();
    assert!(android.ok("/api/me").await["android_app"].is_null());
    assert_eq!(
        android.get("/android/kiwi.apk").await.status,
        StatusCode::NOT_FOUND
    );
    // A new version is picked up.
    write_android_app(&k, b"PK version two", "1.3.0");
    assert_eq!(
        android.ok("/api/me").await["android_app"]["version_name"],
        "1.3.0"
    );
    let r = android.get("/android/kiwi.apk").await;
    assert_eq!(r.status, StatusCode::OK);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn android_devices_are_never_linked_to_an_iphone() {
    let k = TestKiwi::start().await;
    k.devices.add_android(ANDROID_IP);
    let core = fixture_devices("devicectl_list_devices_physical.json")
        .into_iter()
        .find(|c| c.physical && c.reachable())
        .unwrap();
    *k.devices.cores.lock().unwrap() = vec![core.clone()];
    let android = k.android(ANDROID_IP);
    let id = android.ok("/api/me").await["device"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let find = |list: &Value, id: &str| {
        list["devices"]
            .as_array()
            .unwrap()
            .iter()
            .find(|d| d["id"] == id)
            .unwrap()
            .clone()
    };

    // The only device is an Android phone: the iPhone devicectl knows is not linked to it.
    let r = android.post("/api/devices/sync", json!({})).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert!(find(&r.body, &id)["devicectl"].is_null());
    // Nor by hand.
    let path = format!("/api/devices/{id}");
    let link = json!({ "devicectl_id": core.identifier });
    let r = android.send(Method::PATCH, &path, Some(link)).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.body);
    assert_eq!(r.body["error"]["code"], "bad_request");
    assert_eq!(
        r.body["error"]["message"],
        "Only an iPhone or iPad can be linked to a Mac device; this is an Android device."
    );

    // An Android phone linked before Kiwi refused that gets nothing from the iPhone, and can
    // be unlinked.
    let columns = (
        core.identifier.as_str(),
        core.name.as_str(),
        true,
        core.udid.as_deref(),
    );
    k.kiwi.db.set_device_link(&id, Some(columns)).unwrap();
    let r = android.post("/api/devices/sync", json!({})).await;
    let device = find(&r.body, &id);
    assert!(device["devicectl"]["last_sync_at"].is_null(), "{device}");
    assert_ne!(device["os_version_source"], "devicectl");
    assert_eq!(device["model"], "Android");
    let unlink = json!({ "devicectl_id": null });
    let r = android.send(Method::PATCH, &path, Some(unlink)).await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert!(r.body["devicectl"].is_null());

    // The iPhone is linked once it has opened Kiwi.
    let phone = k.phone();
    phone.ok("/api/me").await;
    let r = phone.post("/api/devices/sync", json!({})).await;
    assert_eq!(
        find(&r.body, PHONE_ID)["devicectl"]["id"],
        core.identifier.as_str()
    );
    assert!(find(&r.body, &id)["devicectl"].is_null());
}
