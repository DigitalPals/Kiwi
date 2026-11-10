//! Kiwi is the owner's alone: other people on the tailnet get `403 not_owner`.

mod common;

use common::*;
use reqwest::{Method, StatusCode};
use serde_json::json;

const NOAH_IP: &str = "100.64.0.8";

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_the_owners_devices_may_use_kiwi() {
    let devices = FakeDevices::with_phone();
    devices.add_peer(NOAH_IP, "nNOAH1CNTRL", "noah@example.com", "Noah");
    let k = TestKiwi::start_in(tempfile::tempdir().unwrap(), devices, false).await;

    // The phone belongs to the computer's Tailscale user (a browser on the computer itself:
    // tests/local.rs).
    k.phone().ok("/api/me").await;

    let noah = k.device(NOAH_IP);
    let requests = [
        (Method::GET, "/api/me", None),
        (Method::GET, "/api/apps", None),
        (Method::GET, "/api/library", None),
        (Method::GET, "/api/sources", None),
        (Method::GET, "/api/settings", None),
        (Method::GET, "/api/signing", None),
        (Method::GET, "/api/automation", None),
        (
            Method::POST,
            "/api/sources",
            Some(json!({ "url": "https://example.com/feed.json" })),
        ),
        (
            Method::POST,
            "/api/push/subscribe",
            Some(json!({ "endpoint": "https://web.push.apple.com/noah", "keys": push_keys() })),
        ),
    ];
    for (method, path, body) in requests {
        let r = noah.send(method, path, body).await;
        assert_eq!(r.status, StatusCode::FORBIDDEN, "{path}: {}", r.body);
        assert_eq!(r.body["error"]["code"], "not_owner", "{path}");
        assert_eq!(
            r.body["error"]["message"],
            "This Kiwi belongs to alex@example.com. To use it here, sign in to Tailscale as \
             alex@example.com on this device."
        );
    }
    assert!(k.kiwi.catalog.sources().is_empty());
    noah.ok("/api/health").await;

    // The family endpoints are gone.
    for path in ["/api/family", "/api/me/enroll"] {
        let r = k.phone().send(Method::POST, path, None).await;
        assert_eq!(r.status, StatusCode::NOT_FOUND, "{path}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_owner_is_remembered_while_tailscale_cannot_tell() {
    let k = TestKiwi::start().await;
    k.phone().ok("/api/me").await;
    let dir = k.stop().await;

    // Tailscale no longer answers who owns the Mac: the saved owner still counts.
    let devices = FakeDevices::with_phone();
    *devices.owner.lock().unwrap() = None;
    let k = TestKiwi::start_in(dir, devices, false).await;
    k.phone().ok("/api/me").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nobody_on_the_tailnet_is_the_owner_of_a_tagged_mac() {
    let devices = FakeDevices::with_phone();
    *devices.owner.lock().unwrap() = None;
    let k = TestKiwi::start_on_mac(tempfile::tempdir().unwrap(), devices).await;
    let r = k.phone().get("/api/me").await;
    assert_eq!(r.status, StatusCode::FORBIDDEN);
    assert_eq!(r.body["error"]["code"], "not_owner");
    assert!(
        r.body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("./install.sh --owner")
    );
    // A browser on the Mac itself still works.
    k.browser().ok("/api/me").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn devices_of_someone_else_get_no_updates_or_notifications() {
    // Noah owned this Mac: his iPhone installed an app and turned notifications on.
    let fx = Fixture::start().await;
    let devices = FakeDevices::with_phone();
    devices.add_peer(NOAH_IP, "nNOAH1CNTRL", "noah@example.com", "Noah");
    *devices.owner.lock().unwrap() = Some(kiwi_device::TailscaleUser {
        login: "noah@example.com".into(),
        name: None,
    });
    let k = TestKiwi::start_in(tempfile::tempdir().unwrap(), devices, false).await;
    let noah = k.device(NOAH_IP);
    let source = add_source(&noah, &fx).await;
    install(&k, &noah, &source).await;
    let sub = json!({ "endpoint": "https://web.push.apple.com/noah", "keys": push_keys() });
    let r = noah.post("/api/push/subscribe", sub).await;
    assert_eq!(r.status, StatusCode::NO_CONTENT, "{}", r.body);
    let dir = k.stop().await;

    // Now the Mac is Alex's. A new release is neither prepared nor announced for Noah.
    let k = TestKiwi::start_in(dir, FakeDevices::with_phone(), false).await;
    fx.set_releases(&[("2.0", "2"), ("1.0", "1")]);
    let r = k
        .phone()
        .post(&format!("/api/sources/{source}/refresh"), json!({}))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let builds = k.kiwi.db.builds().unwrap().len();
    let settings = kiwi_server::settings::Settings::default();
    for _ in 0..2 {
        kiwi_server::scheduler::process_updates(&k.kiwi, &settings)
            .await
            .unwrap();
    }
    kiwi_server::scheduler::expiry_warnings(&k.kiwi)
        .await
        .unwrap();
    assert_eq!(k.kiwi.db.builds().unwrap().len(), builds);
    assert!(k.push.sent.lock().unwrap().is_empty());
}
