//! Apps added by IPA link or download page instead of an AltStore source.
mod common;

use common::*;
use reqwest::{Method, StatusCode, header};
use serde_json::{Value, json};

async fn add(api: &Api, url: &str) -> Value {
    let r = api.post("/api/sources", json!({ "url": url })).await;
    assert_eq!(r.status, StatusCode::CREATED, "{}", r.body);
    r.body
}

async fn version(api: &Api, source: &str) -> Value {
    api.ok(&format!("/api/apps/{source}/{BUNDLE}")).await["releases"][0]["version"].clone()
}

#[tokio::test]
async fn an_ipa_link_becomes_an_app_that_updates_when_the_file_changes() {
    let fx = Fixture::start().await;
    let k = TestKiwi::start().await;
    let phone = k.phone();
    let url = fx.put_ipa("Linked.ipa", linked_ipa("1.0", "1"));

    let source = add(&phone, &url).await;
    assert_eq!(source["kind"], "ipa");
    assert_eq!(source["name"], "127.0.0.1");
    assert_eq!(source["subtitle"], "Linked App");
    assert_eq!(source["url"], url.as_str());
    assert_eq!(source["ipa_url"], url.as_str());
    assert_eq!(source["app_count"], 1);
    let id = source["id"].as_str().unwrap().to_owned();
    let again = phone.post("/api/sources", json!({ "url": url })).await;
    assert_eq!(again.status, StatusCode::CONFLICT);

    // The icon comes from the IPA and is served by Kiwi.
    let icon = source["icon_url"].as_str().unwrap();
    assert!(icon.starts_with("/api/icons/"), "{icon}");
    let r = k
        .raw()
        .get(format!("{}{icon}", k.base))
        .header("x-forwarded-for", PHONE_IP)
        .send()
        .await
        .unwrap();
    assert_eq!(r.status(), StatusCode::OK);
    assert_eq!(r.headers()[header::CONTENT_TYPE], "image/png");
    assert!(
        r.headers()[header::CACHE_CONTROL]
            .to_str()
            .unwrap()
            .contains("immutable")
    );
    assert_eq!(r.bytes().await.unwrap(), icon_png());
    let missing = phone.get("/api/icons/..%2Fkiwi.db").await;
    assert_eq!(missing.status, StatusCode::NOT_FOUND);

    let app = &phone.ok("/api/apps").await[0];
    assert_eq!(app["name"], "Linked App");
    assert_eq!(app["source_name"], "127.0.0.1");
    assert_eq!(app["icon_url"], icon);
    assert_eq!(app["state"]["action"], "get");
    let detail = phone.ok(&format!("/api/apps/{id}/{BUNDLE}")).await;
    assert_eq!(detail["releases"][0]["version"], "1.0");
    assert_eq!(detail["releases"][0]["build"], "1");

    // It installs like any other app; the add already downloaded the IPA.
    let prepared = phone
        .post(&format!("/api/apps/{id}/{BUNDLE}/prepare"), json!({}))
        .await;
    assert_eq!(prepared.status, StatusCode::ACCEPTED, "{}", prepared.body);
    let build = wait_build(&phone, prepared.body["id"].as_str().unwrap()).await;
    assert_eq!(build["status"], "ready", "{build}");

    // Unchanged: nothing new. A new file at the same URL: the new version.
    let r = phone
        .post(&format!("/api/sources/{id}/refresh"), json!({}))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.body["error"], Value::Null);
    assert_eq!(version(&phone, &id).await, "1.0");
    fx.put_ipa("Linked.ipa", linked_ipa("1.1", "22"));
    let r = phone
        .post(&format!("/api/sources/{id}/refresh"), json!({}))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.body["error"], Value::Null);
    assert_eq!(version(&phone, &id).await, "1.1");

    // A link that stops working keeps the app and reports the error.
    fx.fail_ipas
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let r = phone
        .post(&format!("/api/sources/{id}/refresh"), json!({}))
        .await;
    assert!(
        r.body["error"].as_str().unwrap().contains("503"),
        "{}",
        r.body
    );
    assert_eq!(version(&phone, &id).await, "1.1");
}

#[tokio::test]
async fn a_download_page_adds_its_source_or_follows_its_ipa_link() {
    let fx = Fixture::start().await;
    let k = TestKiwi::start().await;
    let phone = k.phone();
    let page = format!("{}/page", fx.base);

    // Nothing to add.
    *fx.page.lock().unwrap() = "<html><a href=\"/about\">About</a></html>".into();
    let r = phone.post("/api/sources", json!({ "url": page })).await;
    assert_eq!(r.status, StatusCode::BAD_GATEWAY);
    assert!(
        r.body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("no AltStore source and no IPA"),
        "{}",
        r.body
    );

    // A page with an AltStore button adds that source, even when it links an IPA too.
    let feed = fx.feed_url();
    let encoded: String = url::form_urlencoded::byte_serialize(feed.as_bytes()).collect();
    *fx.page.lock().unwrap() = format!(
        r#"<a href="altstore://source?url={encoded}">Add to AltStore</a>
<a href="/ipa/app-1.0.ipa">IPA</a>"#
    );
    let source = add(&phone, &page).await;
    assert_eq!(source["kind"], "feed");
    assert_eq!(source["url"], feed);
    assert_eq!(source["name"], "Test Source");
    let r = phone.post("/api/sources", json!({ "url": page })).await;
    assert_eq!(r.status, StatusCode::CONFLICT);
    let r = phone
        .send(
            Method::DELETE,
            &format!("/api/sources/{}", source["id"].as_str().unwrap()),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::NO_CONTENT);

    // Without one, its IPA link: refreshes follow the page to new IPAs.
    fx.put_ipa("Paged-2.0.ipa", linked_ipa("2.0", "20"));
    *fx.page.lock().unwrap() = r#"<link rel="apple-touch-icon" href="/icon.png">
<a href="/ipa/Paged-2.0.ipa" download>Download</a> <a href="/ipa/Paged-1.9.ipa">Old</a>"#
        .into();
    let source = add(&phone, &page).await;
    let id = source["id"].as_str().unwrap().to_owned();
    assert_eq!(source["kind"], "ipa");
    assert_eq!(source["url"], page);
    assert_eq!(source["ipa_url"], format!("{}/ipa/Paged-2.0.ipa", fx.base));
    assert_eq!(source["name"], "127.0.0.1");
    assert_eq!(source["diagnostics"].as_array().unwrap().len(), 1);
    assert_eq!(version(&phone, &id).await, "2.0");

    fx.put_ipa("Paged-2.1.ipa", linked_ipa("2.1", "21"));
    *fx.page.lock().unwrap() = r#"<a href="/ipa/Paged-2.1.ipa">Download</a>"#.into();
    let r = phone
        .post(&format!("/api/sources/{id}/refresh"), json!({}))
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body["error"], Value::Null, "{}", r.body);
    assert_eq!(r.body["ipa_url"], format!("{}/ipa/Paged-2.1.ipa", fx.base));
    assert_eq!(r.body["diagnostics"].as_array().unwrap().len(), 0);
    assert_eq!(version(&phone, &id).await, "2.1");

    // A page that drops its IPA link keeps the app and reports it.
    *fx.page.lock().unwrap() = "<p>Gone</p>".into();
    let r = phone
        .post(&format!("/api/sources/{id}/refresh"), json!({}))
        .await;
    assert!(
        r.body["error"]
            .as_str()
            .unwrap()
            .contains("no longer links an IPA"),
        "{}",
        r.body
    );
    assert_eq!(version(&phone, &id).await, "2.1");
}
