//! Requests made on the computer Kiwi runs on (architecture.md "Security"): off macOS only
//! tailscaled may forward a tailnet address, and nothing else on the computer is the owner.
//! The user of each loopback connection comes from [`FakePeers`] instead of
//! `/proc/net/tcp`.

mod common;

use std::{sync::atomic::Ordering, time::Duration};

use common::*;
use kiwi_server::config::LocalTrust;
use reqwest::{Method, StatusCode, header};
use serde_json::json;

/// The address of Kiwi's own computer in the tailnet.
const THIS_IP: &str = "100.64.0.20";

/// `api` from another process: each request on a new connection (a connection's user is
/// looked up once).
fn new_process(api: Api) -> Api {
    Api {
        http: reqwest::Client::builder()
            .no_proxy()
            .pool_max_idle_per_host(0)
            .build()
            .unwrap(),
        ..api
    }
}

fn assert_not_owner(r: &Reply, what: &str) {
    assert_eq!(r.status, StatusCode::FORBIDDEN, "{what}: {}", r.body);
    assert_eq!(r.body["error"]["code"], "not_owner", "{what}");
    assert!(r.headers.get(header::SET_COOKIE).is_none(), "{what}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_tailscaled_speaks_for_the_tailnet() {
    let k = TestKiwi::start().await;
    let listen = k.kiwi.config.listen;

    // tailscale serve (root) forwards the phone's address: the owner's phone.
    let phone = k.phone();
    let me = phone.ok("/api/me").await;
    assert_eq!(me["device"]["id"], PHONE_ID);
    let lookups = k.peers.lookups.lock().unwrap().clone();
    assert_eq!(lookups.len(), 1);
    let (client, server) = lookups[0];
    assert_eq!(server, listen, "Kiwi's end of the connection");
    assert!(client.ip().is_loopback() && client.port() != listen.port());
    // The user of a connection is looked up once.
    phone.ok("/api/library").await;
    phone.ok("/api/sources").await;
    assert_eq!(k.peers.lookups.lock().unwrap().len(), 1);

    // A browser or script on the computer, without X-Forwarded-For: nobody's.
    let devices = k.kiwi.db.devices().unwrap().len();
    let browser = new_process(k.browser());
    let r = browser.get("/api/me").await;
    assert_not_owner(&r, "browser");
    assert_eq!(
        r.body["error"]["message"],
        "Kiwi is used through Tailscale, not on the computer it runs on. Open \
         https://kiwi.test on a phone or computer signed in to Tailscale as alex@example.com."
    );
    // Without a forwarded address nobody is asked.
    assert_eq!(k.peers.lookups.lock().unwrap().len(), 1);
    browser.ok("/api/health").await;
    let r = browser.get("/").await;
    assert_eq!(
        r.status,
        StatusCode::OK,
        "the web app's files are no secret"
    );

    // Another user's process forging the phone's address: not trusted, and Tailscale is not
    // even asked.
    k.peers.set(Ok(Some(1000)));
    let whois = k.devices.whois_calls.load(Ordering::SeqCst);
    let forged = new_process(k.phone());
    let requests = [
        (Method::GET, "/api/me", None),
        (Method::GET, "/api/settings", None),
        (Method::GET, "/api/signing", None),
        (Method::GET, "/android/kiwi.apk", None),
        (
            Method::POST,
            "/api/sources",
            Some(json!({ "url": "https://example.com/feed.json" })),
        ),
        (
            Method::PATCH,
            "/api/settings",
            Some(json!({ "kiwi_builds": true })),
        ),
        (Method::DELETE, "/api/signing/identity", None),
    ];
    for (method, path, body) in requests {
        let r = forged.send(method, path, body).await;
        assert_not_owner(&r, path);
    }
    assert_eq!(k.devices.whois_calls.load(Ordering::SeqCst), whois);
    assert!(k.kiwi.catalog.sources().is_empty());
    assert!(!k.kiwi.settings().kiwi_builds);
    forged.ok("/api/health").await;

    // When the connection can't be found, or the socket tables can't be read: not trusted.
    for lookup in [Ok(None), Err("/proc/self/net/tcp: permission denied")] {
        k.peers.set(lookup);
        assert_not_owner(&forged.get("/api/me").await, &format!("{lookup:?}"));
    }
    assert_eq!(k.devices.whois_calls.load(Ordering::SeqCst), whois);

    // Through tailscaled, but from the computer itself (its own tailnet address): anyone on
    // the computer could have made it, although whois names the owner.
    k.peers.set(Ok(Some(0)));
    k.devices.add_this_computer(THIS_IP);
    let this = new_process(k.device(THIS_IP));
    assert_not_owner(&this.get("/api/me").await, "this computer");
    // An address that isn't a peer: nobody's either.
    assert_not_owner(
        &new_process(k.device("100.64.0.21")).get("/api/me").await,
        "no peer",
    );
    assert_eq!(
        k.kiwi.db.devices().unwrap().len(),
        devices,
        "no devices were made"
    );

    // The phone still works on its connection, and on new ones.
    phone.ok("/api/me").await;
    new_process(k.phone()).ok("/api/me").await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn browser_devices_of_a_computer_are_nobodys() {
    // A Linux Kiwi from before: a script on the computer made a browser device.
    let k = TestKiwi::start_on_mac(tempfile::tempdir().unwrap(), FakeDevices::with_phone()).await;
    let me = k.browser().ok("/api/me").await;
    let id = me["device"]["id"].as_str().unwrap().to_owned();
    let dir = k.stop().await;

    // Elsewhere it gets no updates or notifications; the owner's phone does.
    let k = TestKiwi::start_in(dir, FakeDevices::with_phone(), false).await;
    let device = k.kiwi.db.device(&id).unwrap().unwrap();
    assert!(!kiwi_server::owner::is_owner(&k.kiwi, &device));
    k.phone().ok("/api/me").await;
    let phone = k.kiwi.db.device(PHONE_ID).unwrap().unwrap();
    assert!(kiwi_server::owner::is_owner(&k.kiwi, &phone));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_proxy_uid_can_be_another_user() {
    // Docker with user namespaces: Kiwi sees the sidecar's root as 65534 (--proxy-uid).
    let setup = Setup {
        local: LocalTrust::ProxyUid(65534),
        ..Setup::default()
    };
    let k = TestKiwi::start_setup(tempfile::tempdir().unwrap(), setup).await;
    k.peers.set(Ok(Some(65534)));
    new_process(k.phone()).ok("/api/me").await;
    k.peers.set(Ok(Some(0)));
    assert_not_owner(&new_process(k.phone()).get("/api/me").await, "root");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_mac_is_its_owners_alone() {
    let k = TestKiwi::start_on_mac(tempfile::tempdir().unwrap(), FakeDevices::with_phone()).await;
    // Nobody is asked who made a connection: anything on loopback is trusted.
    k.peers.set(Err("no /proc on a Mac"));
    let me = k.phone().ok("/api/me").await;
    assert_eq!(me["device"]["id"], PHONE_ID);
    // A browser on the Mac is the owner's (a cookie device).
    let r = k.browser().get("/api/me").await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.body["device"]["kind"], "browser");
    assert!(r.headers.get(header::SET_COOKIE).is_some());
    // So is a request from the Mac's own tailnet address, and one whois doesn't know.
    k.devices.add_this_computer(THIS_IP);
    let me = k.device(THIS_IP).ok("/api/me").await;
    assert_eq!(me["device"]["id"], "ts:nKIWIHOST");
    let r = k.device("100.64.0.21").get("/api/me").await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.body["device"]["kind"], "browser");
    assert!(k.peers.lookups.lock().unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn the_public_url_is_learned_after_the_start() {
    // Kiwi started before Tailscale could name the computer (a Tailscale container started
    // after Kiwi's, before its socket existed).
    let setup = Setup {
        public_url: None,
        ..Setup::default()
    };
    let k = TestKiwi::start_setup(tempfile::tempdir().unwrap(), setup).await;
    let listen = format!("http://{}", k.kiwi.config.listen);
    let through_serve = |api: &Api| {
        api.request(Method::GET, "/api/me")
            .header("X-Forwarded-Host", "kiwi.test")
    };
    let phone = k.phone();
    let r = through_serve(&phone).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::MISDIRECTED_REQUEST);
    assert_eq!(phone.ok("/api/me").await["server"]["public_url"], listen);
    // The message for a request on the computer can't name the address yet.
    let r = new_process(k.browser()).get("/api/me").await;
    assert!(
        r.body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("Open its Tailscale address on"),
        "{}",
        r.body
    );

    // Kiwi keeps asking until Tailscale names the computer.
    let mut answers = vec![
        Ok(Some("https://kiwi.test".to_owned())),
        Ok(None),
        Err("tailscaled: connection refused".to_owned()),
    ];
    let asked = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let learning = kiwi_server::learn_public_url(
        k.kiwi.clone(),
        {
            let asked = asked.clone();
            move || {
                asked.fetch_add(1, Ordering::SeqCst);
                let answer = answers.pop().unwrap();
                async move { answer }
            }
        },
        Duration::from_millis(1),
    );
    tokio::time::timeout(Duration::from_secs(5), learning)
        .await
        .unwrap();
    assert_eq!(asked.load(Ordering::SeqCst), 3);

    let r = through_serve(&phone).send().await.unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(
        phone.ok("/api/me").await["server"]["public_url"],
        "https://kiwi.test"
    );
    assert_eq!(k.kiwi.default_public_url(), "https://kiwi.test");
    assert_eq!(*k.push.subjects.lock().unwrap(), ["https://kiwi.test"]);
    // Other names are still refused.
    let r = phone
        .request(Method::GET, "/api/me")
        .header("X-Forwarded-Host", "evil.test")
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::MISDIRECTED_REQUEST);

    // Asking stops when Kiwi shuts down.
    let never = kiwi_server::learn_public_url(
        k.kiwi.clone(),
        || async { Ok(None) },
        Duration::from_millis(1),
    );
    let never = tokio::spawn(never);
    tokio::time::sleep(Duration::from_millis(20)).await;
    k.kiwi.cancel.cancel();
    tokio::time::timeout(Duration::from_secs(5), never)
        .await
        .unwrap()
        .unwrap();
}
