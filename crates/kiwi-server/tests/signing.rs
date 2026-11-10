//! `/api/signing`: uploaded identities and profiles. On Linux the real signer (the files
//! backend) signs a build end to end.

mod common;

use base64::{Engine, engine::general_purpose::STANDARD as B64};
use common::*;
use kiwi_signer::Backend;
use reqwest::{Method, StatusCode};
use serde_json::json;

fn wildcard_profile(certificate: Option<Vec<u8>>, expires: &'static str) -> Vec<u8> {
    use kiwi_signer::testutil::{ProfileSpec, profile_bytes};
    profile_bytes(&ProfileSpec {
        certificate,
        expires,
        ..ProfileSpec::wildcard()
    })
}

#[tokio::test]
async fn keychain_hosts_refuse_identity_uploads_but_take_profiles() {
    let k = TestKiwi::start().await;
    *k.signer.backend.lock().unwrap() = Backend::Keychain;
    let api = k.phone();
    let r = api
        .post(
            "/api/signing/identity",
            json!({ "p12": B64.encode(b"p12"), "password": "secret" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT, "{}", r.body);
    assert_eq!(r.body["error"]["code"], "conflict");
    assert!(
        r.body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("login keychain")
    );
    let r = api
        .send(Method::DELETE, "/api/signing/identity", None)
        .await;
    assert_eq!(r.status, StatusCode::CONFLICT);

    // Profiles can be uploaded on every host.
    let profile = wildcard_profile(None, "2099-01-01T00:00:00Z");
    let r = api
        .post(
            "/api/signing/profiles",
            json!({ "profile": B64.encode(&profile) }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert_eq!(r.body["backend"], "keychain");
    let stored = k.kiwi.paths.signing.profiles_dir().join(format!(
        "{}.mobileprovision",
        kiwi_signer::testutil::ProfileSpec::wildcard().uuid
    ));
    assert_eq!(std::fs::read(stored).unwrap(), profile);
    k.stop().await;
}

#[tokio::test]
async fn validates_uploads() {
    let k = TestKiwi::start().await;
    *k.signer.backend.lock().unwrap() = Backend::Files;
    // The backend is read when signing is scanned; the first scan ran with this system's.
    k.kiwi.rescan_signing().await;
    let api = k.phone();
    let post = |path: &'static str, body: serde_json::Value| {
        let api = api.clone();
        async move { api.post(path, body).await }
    };

    let r = post("/api/signing/profiles", json!({ "profile": "not base64!" })).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.body["error"]["message"], "profile must be base64.");
    let r = post(
        "/api/signing/profiles",
        json!({ "profile": B64.encode(b"not a profile") }),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(
        r.body["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("Invalid provisioning profile")
    );
    let expired = wildcard_profile(None, "2020-01-01T00:00:00Z");
    let r = post(
        "/api/signing/profiles",
        json!({ "profile": B64.encode(&expired) }),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(
        r.body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("expired")
    );
    let r = post(
        "/api/signing/profiles",
        json!({ "profile": "", "extra": 1 }),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);

    // Larger than any profile.
    let huge = "A".repeat(2 * 1024 * 1024);
    let r = post("/api/signing/profiles", json!({ "profile": huge })).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.body["error"]["message"], "Request body is too large.");
    let r = post(
        "/api/signing/identity",
        json!({ "p12": "A".repeat(200 * 1024), "password": "" }),
    )
    .await;
    assert_eq!(r.body["error"]["message"], "Request body is too large.");

    // A profile nested deep enough to overflow a stack is refused, and Kiwi keeps running.
    let deep = kiwi_signer::testutil::nested_profile(60_000);
    let r = post(
        "/api/signing/profiles",
        json!({ "profile": B64.encode(&deep) }),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST, "{}", r.body);
    assert_eq!(
        r.body["error"]["message"],
        "Invalid provisioning profile: nested more than 64 levels deep"
    );
    assert_eq!(api.ok("/api/signing").await["backend"], "files");

    // The identity's body is read like other JSON bodies.
    let r = post(
        "/api/signing/identity",
        json!({ "p12": "", "pasword": "x" }),
    )
    .await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(
        r.body["error"]["message"]
            .as_str()
            .unwrap()
            .starts_with("Invalid request body: unknown field `pasword`")
    );
    let r = api.send(Method::POST, "/api/signing/identity", None).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert!(
        r.body["error"]["message"]
            .as_str()
            .unwrap()
            .contains("missing field `p12`"),
        "{}",
        r.body
    );

    let r = post(
        "/api/signing/identity",
        json!({ "p12": B64.encode(b"not a p12"), "password": "hunter2" }),
    )
    .await;
    // A Mac signs with its keychain and refuses an uploaded identity before reading it.
    let refused = if cfg!(target_os = "macos") {
        StatusCode::CONFLICT
    } else {
        StatusCode::BAD_REQUEST
    };
    assert_eq!(r.status, refused, "{}", r.body);
    assert!(!r.body.to_string().contains("hunter2"));

    let r = api
        .send(Method::DELETE, "/api/signing/profiles/unknown-uuid", None)
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    let r = api
        .send(Method::DELETE, "/api/signing/identity", None)
        .await;
    assert_eq!(r.status, StatusCode::NOT_FOUND);
    assert!(!k.kiwi.paths.signing.path().join("identity").exists());
    k.stop().await;
}

/// The real signer of this system with its files in the test's data directory.
#[cfg(not(target_os = "macos"))]
async fn files_kiwi() -> TestKiwi {
    let dir = tempfile::tempdir().unwrap();
    let signer = kiwi_server::signing::SystemSigner {
        dir: kiwi_server::signing_dir(&dir.path().join("data")),
    };
    TestKiwi::start_with_signer(dir, std::sync::Arc::new(signer)).await
}

#[cfg(not(target_os = "macos"))]
#[tokio::test]
async fn uploads_an_identity_and_profiles_on_files_hosts() {
    use kiwi_signer::testutil::{TEAM, signing_identity};
    use std::os::unix::fs::PermissionsExt;

    let k = files_kiwi().await;
    let api = k.phone();
    let signing = api.ok("/api/signing").await;
    assert_eq!(signing["backend"], "files");
    assert_eq!(signing["identities"], json!([]));
    assert!(
        signing["error"]
            .as_str()
            .unwrap()
            .starts_with("No signing identity was uploaded")
    );

    let identity = signing_identity(TEAM, "correct horse");
    let upload = |password: &str| json!({ "p12": B64.encode(&identity.p12), "password": password });
    let r = api.post("/api/signing/identity", upload("wrong")).await;
    assert_eq!(r.status, StatusCode::BAD_REQUEST);
    assert_eq!(r.body["error"]["message"], "The password is wrong");

    let r = api
        .post("/api/signing/identity", upload("correct horse"))
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let text = r.body.to_string();
    assert!(!text.contains("correct horse") && !text.contains(&B64.encode(&identity.p12)));
    let ids = r.body["identities"].as_array().unwrap();
    assert_eq!(ids.len(), 1);
    assert_eq!(ids[0]["team_id"], TEAM);
    assert_eq!(
        ids[0]["name"],
        format!("Apple Development: Kiwi Test ({TEAM})")
    );
    assert!(
        r.body["error"]
            .as_str()
            .unwrap()
            .contains("No usable wildcard provisioning profile")
    );
    let files = k.kiwi.paths.signing.path();
    let mode = |name: &str| {
        std::fs::metadata(files.join(name))
            .unwrap()
            .permissions()
            .mode()
            & 0o777
    };
    // The .p12 and its password are kept together in one file.
    assert_eq!((mode(""), mode("identity")), (0o700, 0o600));
    assert!(!files.join("identity.p12").exists() && !files.join("identity.password").exists());

    let profile = wildcard_profile(Some(identity.certificate.clone()), "2099-01-01T00:00:00Z");
    let r = api
        .post(
            "/api/signing/profiles",
            json!({ "profile": B64.encode(&profile) }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    assert!(r.body["error"].is_null(), "{}", r.body);
    let uuid = kiwi_signer::testutil::ProfileSpec::wildcard().uuid;
    assert_eq!(r.body["profiles"][0]["uuid"], uuid);
    assert_eq!(r.body["profiles"][0]["uploaded"], true);
    assert_eq!(r.body["profiles"][0]["usable"], true);
    assert_eq!(r.body["active"]["identity_sha1"], ids[0]["sha1"]);
    assert_eq!(api.ok("/api/signing").await, r.body);

    let r = api
        .send(
            Method::DELETE,
            &format!("/api/signing/profiles/{uuid}"),
            None,
        )
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body["profiles"], json!([]));
    let r = api
        .send(Method::DELETE, "/api/signing/identity", None)
        .await;
    assert_eq!(r.status, StatusCode::OK);
    assert_eq!(r.body["identities"], json!([]));
    assert!(!files.join("identity").exists());
    k.stop().await;
}

/// An app whose executable is a real (unsigned) Mach-O, so it can really be signed.
#[cfg(not(target_os = "macos"))]
fn signable_ipa(version: &str) -> Vec<u8> {
    use kiwi_signer::testutil::{Zip, ios_macho};
    let info = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>{BUNDLE}</string>
<key>CFBundleExecutable</key><string>Kiwi</string>
<key>CFBundleShortVersionString</key><string>{version}</string>
<key>CFBundleVersion</key><string>1</string>
<key>MinimumOSVersion</key><string>16.0</string>
<key>CFBundleSupportedPlatforms</key><array><string>iPhoneOS</string></array>
<key>UIDeviceFamily</key><array><integer>1</integer></array>
</dict></plist>"#
    );
    Zip::new()
        .file("Payload/Kiwi.app/Info.plist", info.as_bytes(), 0o644)
        .file("Payload/Kiwi.app/Kiwi", &ios_macho(false), 0o755)
        .file(
            "Payload/Kiwi.app/Frameworks/Lib.dylib",
            &ios_macho(true),
            0o755,
        )
        .finish()
}

#[cfg(not(target_os = "macos"))]
#[tokio::test]
async fn files_hosts_sign_builds() {
    use kiwi_signer::testutil::{TEAM, signing_identity};
    use std::io::Read;

    let fixture = Fixture::start().await;
    let ipa = signable_ipa("1.0");
    let url = fixture.put_ipa("signable-1.0.ipa", ipa.clone());
    {
        let mut feed = fixture.feed.lock().unwrap();
        let release = &mut feed["apps"][0]["versions"][0];
        release["downloadURL"] = url.into();
        release["size"] = ipa.len().into();
    }
    let k = files_kiwi().await;
    let api = k.phone();
    let identity = signing_identity(TEAM, "");
    let r = api
        .post(
            "/api/signing/identity",
            json!({ "p12": B64.encode(&identity.p12), "password": "" }),
        )
        .await;
    assert_eq!(r.status, StatusCode::OK, "{}", r.body);
    let profile = wildcard_profile(Some(identity.certificate.clone()), "2099-01-01T00:00:00Z");
    let r = api
        .post(
            "/api/signing/profiles",
            json!({ "profile": B64.encode(&profile) }),
        )
        .await;
    assert!(r.body["error"].is_null(), "{}", r.body);

    let source = add_source(&api, &fixture).await;
    let build = install(&k, &api, &source).await;
    assert_eq!(build["signed_bundle_id"], format!("{BUNDLE}.{TEAM}"));

    // The served IPA is signed: sealed resources, the profile and the certificate.
    let ota = ota_path(build["install_url"].as_str().unwrap());
    let bytes = k
        .raw()
        .get(format!("{}{ota}/app.ipa", k.base))
        .header("User-Agent", INSTALLER_UA)
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    let mut zip = zip::ZipArchive::new(std::io::Cursor::new(bytes.to_vec())).unwrap();
    let mut read = |name: &str| {
        let mut out = Vec::new();
        zip.by_name(name).unwrap().read_to_end(&mut out).unwrap();
        out
    };
    assert_eq!(read("Payload/Kiwi.app/embedded.mobileprovision"), profile);
    let seal = read("Payload/Kiwi.app/_CodeSignature/CodeResources");
    assert!(String::from_utf8_lossy(&seal).contains("Frameworks/Lib.dylib"));
    let main = read("Payload/Kiwi.app/Kiwi");
    assert!(
        main.windows(identity.certificate.len())
            .any(|w| w == identity.certificate)
    );
    k.stop().await;
}
