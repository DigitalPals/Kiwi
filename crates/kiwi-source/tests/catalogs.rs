use kiwi_source::*;
use serde_json::json;

const BLOB: &str =
    "https://github.com/mrdrvt99/Altstore-Repository/blob/main/youmod.json?utm_source=chatgpt.com";
const RAW: &str = "https://raw.githubusercontent.com/mrdrvt99/Altstore-Repository/main/youmod.json";
const DRIFTYWINDS_RAW: &str =
    "https://raw.githubusercontent.com/driftywinds/driftywinds.github.io/master/AltStore/apps.json";
fn base() -> Url {
    Url::parse("https://example.test/catalog/feed.json?catalog=public").unwrap()
}
fn catalog(value: serde_json::Value) -> Catalog {
    parse_catalog(&serde_json::to_vec(&value).unwrap(), &base()).unwrap()
}
fn app(releases: serde_json::Value) -> App {
    catalog(json!({"name":"Test","apps":[{"name":"App","bundleIdentifier":"test.app","versions":releases}]})).apps.remove(0)
}
fn release(version: &str, build: Option<&str>) -> Release {
    app(json!([{"version":version,"buildVersion":build,"downloadURL":"/artifact"}]))
        .releases
        .remove(0)
}

#[test]
fn local_patch_revisions_order_updates_without_replacing_apple_versions() {
    let mut old = release("21.43.5", Some("21.43.5"));
    old.revision = Some(2);
    let mut current = old.clone();
    current.revision = Some(4);
    current.download_url = Url::parse("https://example.test/corrected.ipa").unwrap();
    let mut a = app(json!([{"version":"21.43.5","downloadURL":"/artifact"}]));
    a.releases = vec![current.clone(), old.clone()];
    assert_eq!(release_change(&a, 0, &old), ReleaseChange::Newer);
    assert_eq!(release_change(&a, 1, &current), ReleaseChange::Older);
    assert!(!old.same_release(&current));
    assert_eq!(release_change(&a, 0, &current), ReleaseChange::Same);
    let restored: Release =
        serde_json::from_value(serde_json::to_value(&current).unwrap()).unwrap();
    assert_eq!(restored, current);
    let broken = release("21.43.5", Some("1.0.2"));
    assert_eq!(release_change(&a, 0, &broken), ReleaseChange::Newer);
    let unknown = release("21.43.5", Some("21.43.5"));
    assert_eq!(release_change(&a, 0, &unknown), ReleaseChange::Ambiguous);
    a.releases[0].version = "21.42.1".into();
    a.releases[0].revision = Some(999);
    assert_eq!(release_change(&a, 0, &old), ReleaseChange::Older);
}

#[test]
fn exact_github_input_deduplicates_with_raw_and_tracking() {
    let SourceInput::Canonical(blob) = source_input(BLOB).unwrap() else {
        panic!()
    };
    let SourceInput::Canonical(raw) = source_input(RAW).unwrap() else {
        panic!()
    };
    assert_eq!(blob, raw);
    assert_eq!(source_key(&blob), source_key(&raw));
    assert_eq!(
        source_input(&format!("{RAW}?utm_source=tracker&raw=true#foo")).unwrap(),
        SourceInput::Canonical(raw)
    );
}
#[test]
fn preserves_meaningful_query_parameters_and_rejects_credentials_and_schemes() {
    let url = "https://catalog.example.test/list?token=signed%2Bvalue&utm_source=meaningful";
    assert_eq!(
        source_input(url).unwrap(),
        SourceInput::Canonical(Url::parse(url).unwrap())
    );
    let raw = format!("{RAW}?token=signed%2Bvalue&utm_source=tracker");
    assert!(
        matches!(source_input(&raw).unwrap(),SourceInput::Canonical(u) if u.query()==Some("token=signed%2Bvalue"))
    );
    for url in [
        "file:///etc/passwd",
        "javascript:alert(1)",
        "https://user:pass@example.test/feed",
        "ftp://example.test/feed",
    ] {
        assert!(source_input(url).is_err());
    }
    // Plain HTTP is upgraded and a missing scheme means HTTPS; loopback keeps HTTP.
    for url in [
        "http://example.test/feed",
        "example.test/feed",
        " example.test/feed ",
    ] {
        assert_eq!(
            source_input(url).unwrap(),
            SourceInput::Canonical(Url::parse("https://example.test/feed").unwrap())
        );
    }
    assert_eq!(
        source_input("http://127.0.0.1:8080/feed").unwrap(),
        SourceInput::Canonical(Url::parse("http://127.0.0.1:8080/feed").unwrap())
    );
    assert!(network_url("https://cdn.example.test/artifact?signature=keep-me").is_ok());
}
#[test]
fn encoded_paths_and_ambiguous_refs_are_not_guessed() {
    assert_eq!(
        source_input("https://github.com/org/repo/blob/feature%2Fbranch/a%20b.json?utm_source=x")
            .unwrap(),
        SourceInput::Canonical(
            Url::parse("https://raw.githubusercontent.com/org/repo/feature%2Fbranch/a%20b.json")
                .unwrap()
        )
    );
    assert!(matches!(
        source_input("https://github.com/org/repo/blob/feature/branch/a.json").unwrap(),
        SourceInput::Github { .. }
    ));
    assert!(matches!(
        source_input("https://github.com/org/repo/blob/main/catalog/a.json").unwrap(),
        SourceInput::Github { .. }
    ));
}
#[test]
fn pinned_publisher_feeds_prefer_modern_records_and_remain_separate() {
    let you = parse_catalog(
        include_bytes!("fixtures/youmod.json"),
        &Url::parse(RAW).unwrap(),
    )
    .unwrap();
    let music = parse_catalog(
        include_bytes!("fixtures/ytmusic.json"),
        &Url::parse(RAW).unwrap(),
    )
    .unwrap();
    assert_eq!(you.name, "YouMod");
    assert_eq!(music.name, "YTMusicUltimate");
    for source in [&you, &music] {
        assert_eq!(source.apps.len(), 1);
        assert_eq!(source.apps[0].releases[0].version, "1.3.0");
        assert_eq!(source.apps[0].releases[0].min_os.as_deref(), Some("17.0"));
    }
    assert_eq!(you.identifier, music.identifier); // Publisher collisions are metadata, not store keys.
    assert_ne!(
        source_key(&Url::parse(RAW).unwrap()),
        source_key(&Url::parse(&RAW.replace("youmod", "ytmusic")).unwrap())
    );
}
#[test]
fn driftywinds_legacy_feed_parses_without_warnings() {
    // Publisher feed captured 2026-10-29 from DRIFTYWINDS_RAW.
    let url = Url::parse(DRIFTYWINDS_RAW).unwrap();
    assert_eq!(
        source_input(DRIFTYWINDS_RAW).unwrap(),
        SourceInput::Canonical(url.clone())
    );
    let catalog = parse_catalog(include_bytes!("fixtures/driftywinds.json"), &url).unwrap();
    assert_eq!(catalog.name, "driftywinds' AltStore Repo");
    assert_eq!(catalog.identifier.as_deref(), Some("com.dw.source"));
    assert_eq!(catalog.apps.len(), 10);
    assert!(catalog.diagnostics.is_empty(), "{:?}", catalog.diagnostics);
    for app in &catalog.apps {
        assert!(app.unavailable.is_none(), "{}", app.name);
        assert_eq!(app.releases.len(), 1, "{}", app.name);
        assert_eq!(app.newest_eligible(None), Some(0));
    }
    let app = &catalog.apps[0];
    assert_eq!(app.name, "YouMod + Tweaks (driftywinds)");
    assert_eq!(app.bundle_id, "com.google.ios.youtube");
    assert_eq!(app.developer.as_deref(), Some("Tonwalter888"));
    assert!(
        app.description
            .as_deref()
            .unwrap()
            .contains("A modified version of Youtube")
    );
    assert_eq!(app.screenshots.len(), 3);
    assert_eq!(app.tint_color.as_deref(), Some("#E22A41"));
    assert_eq!(app.permissions.entitlements, ["com.apple.developer.siri"]);
    let release = &app.releases[0];
    assert_eq!(release.version, "21.43.5");
    assert_eq!(release.date.as_deref(), Some("2026-10-29"));
    assert_eq!(release.size, Some(130_097_628));
    assert_eq!(
        release.notes.as_deref(),
        Some("• Updated YouTube to 21.43.5\n • Tweak is using 2.0.0")
    );
    assert_eq!(
        release.download_url.as_str(),
        "https://github.com/ramramzz/descfsr/releases/download/v0.37/f61b83c6-8b53-41a9-8c96-57f874c853YM.ipa"
    );
}
#[test]
fn legacy_release_notes_do_not_use_the_app_description() {
    for notes in [json!(null), json!(""), json!("Release notes")] {
        let catalog = catalog(json!({"name":"Legacy","apps":[{
            "name":"App","bundleIdentifier":"test.app","version":"1","downloadURL":"/app.ipa",
            "localizedDescription":"App description","versionDescription":notes
        }]}));
        let app = &catalog.apps[0];
        assert_eq!(app.description.as_deref(), Some("App description"));
        assert_eq!(
            app.releases[0].notes.as_deref(),
            notes.as_str().filter(|s| !s.is_empty())
        );
        assert!(catalog.diagnostics.is_empty(), "{:?}", catalog.diagnostics);
    }
}
#[test]
fn json5_legacy_fallback_optional_forms_and_diagnostics() {
    let input=br#"{name:'JSON5',apps:[{name:'App',bundleIdentifier:'test.app',version:'legacy',downloadURL:'../download?signature=yes',
      versions:[{version:'broken'}],screenshots:{iphone:['one.png',{imageURL:'two.png',width:300}],ipad:[{imageURL:'ipad.png'}]},
      iconURL:'javascript:bad',appPermissions:{future:true},apiVersion:'whatever'}, {name:'Broken'},],news:[],}"#;
    let catalog = parse_catalog(input, &base()).unwrap();
    let app = &catalog.apps[0];
    assert_eq!(app.releases[0].version, "legacy");
    assert_eq!(
        app.releases[0].download_url.as_str(),
        "https://example.test/download?signature=yes"
    );
    assert_eq!(app.screenshots.len(), 3);
    assert!(app.icon_url.is_none());
    assert!(catalog.diagnostics.len() >= 3);
    assert!(
        catalog
            .diagnostics
            .iter()
            .any(|d| d.contains("using legacy flat"))
    );
    assert!(app.description.is_none());
}
#[test]
fn deeply_nested_feeds_are_rejected_without_overflowing_the_stack() {
    // Feeds are parsed on tokio's blocking threads, which have 2 MiB stacks.
    let parse = |input: String| {
        std::thread::Builder::new()
            .stack_size(2 * 1024 * 1024)
            .spawn(move || parse_catalog(input.as_bytes(), &base()).map(|c| c.name))
            .unwrap()
            .join()
            .expect("parser thread")
    };
    // Release builds parse this far and then overflowed converting the parse tree.
    let closed = format!(
        "{{name:'Deep',apps:[],x:{}1{}}}",
        "[".repeat(20_000),
        "]".repeat(20_000)
    );
    for input in [closed, "[".repeat(50_000), "{a:".repeat(20_000)] {
        let err = parse(input).unwrap_err();
        assert!(err.to_string().contains("nested more than 128"), "{err}");
    }
    // The limit still lets JSON5 parse as deep as serde_json would.
    let deep = format!(
        "{{name:'Deep',apps:[],x:{}1{}}}",
        "[".repeat(126),
        "]".repeat(126)
    );
    assert_eq!(parse(deep).unwrap(), "Deep");
}
#[test]
fn validates_required_records_and_urls_but_allows_empty_and_unavailable_catalogs() {
    for invalid in [
        br#"<html>login</html>"#.as_slice(),
        br#"{"error":"API limit"}"#,
        br#"{"name":"bad","apps":[{}]}"#,
    ] {
        assert!(parse_catalog(invalid, &base()).is_err());
    }
    let empty = catalog(json!({"name":"Empty","apps":[]}));
    assert!(empty.apps.is_empty());
    assert!(!empty.diagnostics.is_empty());
    let unavailable = catalog(
        json!({"name":"Test","apps":[{"name":"PAL","bundleIdentifier":"test.pal","marketplaceID":"123"}]}),
    );
    assert!(
        unavailable.apps[0]
            .unavailable
            .as_deref()
            .unwrap()
            .contains("PAL")
    );
    let app = app(
        json!([{"version":"1","downloadURL":"file:///tmp/bad"},{"version":"2","downloadURL":"/good","sha256":"bad"},{"version":"3","downloadURL":"/good"}]),
    );
    assert_eq!(app.releases.len(), 1);
    assert_eq!(app.releases[0].version, "3");
}
#[test]
fn bundle_collisions_only_within_the_same_catalog_are_rejected() {
    let item =
        json!({"name":"App","bundleIdentifier":"test.app","version":"1","downloadURL":"/ipa"});
    let first = catalog(json!({"name":"A","apps":[item.clone(),item.clone()]}));
    let second = catalog(json!({"name":"B","apps":[item]}));
    assert_eq!(first.apps.len(), 1);
    assert_eq!(second.apps.len(), 1);
    assert!(first.diagnostics.iter().any(|d| d.contains("duplicate")));
}
#[test]
fn publisher_order_and_eligible_versions_are_deterministic() {
    let app = app(json!([
        {"version":"release-c","downloadURL":"/c","minOSVersion":"18"},
        {"version":"release-b","downloadURL":"/b","minOSVersion":"16","maxOSVersion":"17.9"},
        {"version":"release-a","downloadURL":"/a","platform":"tvos"}
    ]));
    assert_eq!(app.newest_eligible(None), Some(0));
    let d = DeviceProfile {
        os_version: "17.0.1".into(),
        platform: "ios".into(),
        family: 1,
    };
    assert_eq!(app.newest_eligible(Some(&d)), Some(1));
    assert_eq!(
        release_change(&app, 0, &app.releases[1]),
        ReleaseChange::Newer
    );
    assert_eq!(
        release_change(&app, 1, &app.releases[0]),
        ReleaseChange::Older
    );
    assert_eq!(
        compare_numeric("1.10", "1.9"),
        Some(std::cmp::Ordering::Greater)
    );
    assert_eq!(
        compare_numeric("1.0", "1.0.0"),
        Some(std::cmp::Ordering::Equal)
    );
}
#[test]
fn avoids_false_updates_downgrades_and_endless_artifact_changes() {
    let mut app = app(
        json!([{"version":"1.10","buildVersion":"10","downloadURL":"/ipa","date":"2025-01-01"}]),
    );
    let installed = app.releases[0].clone();
    app.releases[0].date = Some("2030-01-01".into());
    app.releases[0].notes = Some("changed notes".into());
    assert_eq!(release_change(&app, 0, &installed), ReleaseChange::Same);
    app.releases[0].download_url = Url::parse("https://example.test/other").unwrap();
    assert_eq!(
        release_change(&app, 0, &installed),
        ReleaseChange::Ambiguous
    );
    assert_eq!(
        release_change(&app, 0, &release("1.9", Some("999"))),
        ReleaseChange::Newer
    );
    assert_eq!(
        release_change(&app, 0, &release("2.0", Some("1"))),
        ReleaseChange::Older
    );
    assert_eq!(
        release_change(&app, 0, &release("1.10", Some("9"))),
        ReleaseChange::Newer
    );
    assert_eq!(
        release_change(&app, 0, &release("1.10", Some("11"))),
        ReleaseChange::Older
    );
    assert_eq!(
        release_change(&app, 0, &release("uncomparable", None)),
        ReleaseChange::Ambiguous
    );
}
#[test]
fn searches_names_developers_and_bundle_ids_without_account_or_device() {
    let catalog = parse_catalog(include_bytes!("fixtures/ytmusic.json"), &base()).unwrap();
    let app = &catalog.apps[0];
    for q in ["ytmusic", "DAYANCH96", "com.google.ios"] {
        assert!(app.matches(q));
    }
    assert!(!app.matches("unrelated"));
}
#[test]
fn real_ytlite_feed_parses_releases_and_metadata_in_publisher_order() {
    let url = Url::parse(
        "https://raw.githubusercontent.com/mrdrvt99/Altstore-Repository/main/ytlite.json",
    )
    .unwrap();
    let catalog = parse_catalog(include_bytes!("fixtures/ytlite.json"), &url).unwrap();
    assert_eq!(catalog.name, "YouTubePlus (YTLite)");
    assert_eq!(catalog.identifier.as_deref(), Some("com.mrdrvt99.ytlite"));
    assert!(catalog.diagnostics.is_empty(), "{:?}", catalog.diagnostics);
    let app = &catalog.apps[0];
    assert_eq!(catalog.apps.len(), 1);
    assert_eq!(app.name, "YouTubePlus");
    assert_eq!(app.bundle_id, "com.google.ios.youtube");
    assert_eq!(app.developer.as_deref(), Some("dayanch96"));
    assert_eq!(app.tint_color.as_deref(), Some("#FF0000"));
    assert_eq!(app.category.as_deref(), Some("photo-video"));
    assert!(app.unavailable.is_none());
    let versions: Vec<_> = app.releases.iter().map(|r| r.version.as_str()).collect();
    assert_eq!(
        versions,
        ["21.43.5", "21.33.6", "21.24.3", "20.21.6", "19.20.2"]
    );
    let newest = &app.releases[0];
    assert_eq!(newest.size, Some(150_260_037));
    assert_eq!(newest.min_os.as_deref(), Some("17.0"));
    assert_eq!(newest.date.as_deref(), Some("2026-10-27"));
    assert!(newest.notes.as_deref().unwrap().contains("21.43.5"));
    assert_eq!(
        newest.download_url.as_str(),
        "https://github.com/mrdrvt99/YouProEXTRA/releases/download/ytl-ipa6/YouTubePlus_21.43.5_6.0b2.ipa"
    );
    assert_eq!(app.permissions.entitlements.len(), 12);
    assert!(
        app.permissions
            .entitlements
            .iter()
            .any(|e| e == "com.apple.security.application-groups")
    );
    assert_eq!(app.permissions.privacy.len(), 7);
    assert!(app.permissions.privacy.iter().any(|(k, d)| {
        k == "NSCameraUsageDescription" && d == "Used to record and upload videos to YouTube."
    }));
    let ios = |os: &str| DeviceProfile {
        os_version: os.into(),
        platform: "ios".into(),
        family: 1,
    };
    assert_eq!(app.newest_eligible(None), Some(0));
    assert_eq!(app.newest_eligible(Some(&ios("27.0.1"))), Some(0));
    assert_eq!(app.newest_eligible(Some(&ios("16.7"))), Some(3));
    assert_eq!(app.newest_eligible(Some(&ios("13.0"))), None);
    assert!(
        app.releases[0]
            .incompatibility(&ios("16.7"))
            .unwrap()
            .contains("17.0")
    );
    assert_eq!(
        release_change(app, 0, &app.releases[1]),
        ReleaseChange::Newer
    );

    // Sources list iOS apps: a release that names no platform is not for Android.
    let android = DeviceProfile {
        os_version: String::new(),
        platform: "android".into(),
        family: 1,
    };
    assert!(app.releases[0].platforms.is_empty());
    assert!(app.supports_platform("ios") && !app.supports_platform("android"));
    assert_eq!(app.newest_eligible(Some(&android)), None);
    assert_eq!(
        app.releases[0].incompatibility(&android).as_deref(),
        Some("Made for iPhone and iPad")
    );
    // An Android release names its platform and no OS version.
    let mut built = app.releases[0].clone();
    built.platforms = vec!["Android".into()];
    built.min_os = None;
    assert_eq!(built.incompatibility(&android), None);
    assert_eq!(
        built.incompatibility(&ios("27.0")).as_deref(),
        Some("Made for Android devices")
    );
}
#[test]
fn permission_forms_merge_and_malformed_entries_become_diagnostics() {
    let catalog = catalog(json!({"name":"Perms","apps":[
        {"name":"Current","bundleIdentifier":"test.current","version":"1","downloadURL":"/a","appPermissions":{
            "entitlements":["aps-environment",{"name":"keychain-access-groups"},"aps-environment",7,{"other":"x"}," "],
            "privacy":[{"name":"NSCameraUsageDescription","usageDescription":"Camera"},{"name":"NSMissing"},"bad"]},
         "permissions":[{"type":"photos","usageDescription":"Photos"},{"type":"NSCameraUsageDescription","usageDescription":"dup"},{"usageDescription":"no type"}]},
        {"name":"Map","bundleIdentifier":"test.map","version":"1","downloadURL":"/a",
         "appPermissions":{"entitlements":[],"privacy":{"NSMicrophoneUsageDescription":"Mic","NSBad":5}}},
        {"name":"Legacy","bundleIdentifier":"test.legacy","version":"1","downloadURL":"/a",
         "permissions":[{"type":"background-audio","usageDescription":"Plays in background"}]},
        {"name":"Wrong","bundleIdentifier":"test.wrong","version":"1","downloadURL":"/a",
         "appPermissions":{"entitlements":"aps-environment","privacy":true},"permissions":{"type":"x"}},
        {"name":"Scalar","bundleIdentifier":"test.scalar","version":"1","downloadURL":"/a","appPermissions":"all"}
    ]}));
    assert_eq!(catalog.apps.len(), 5);
    let [current, map, legacy, wrong, scalar] = &catalog.apps[..] else {
        panic!()
    };
    assert_eq!(
        current.permissions,
        Permissions {
            entitlements: vec!["aps-environment".into(), "keychain-access-groups".into()],
            privacy: vec![
                ("NSCameraUsageDescription".into(), "Camera".into()),
                ("photos".into(), "Photos".into()),
            ],
        }
    );
    assert_eq!(
        map.permissions.privacy,
        [("NSMicrophoneUsageDescription".into(), "Mic".into())]
    );
    assert_eq!(
        legacy.permissions.privacy,
        [("background-audio".into(), "Plays in background".into())]
    );
    assert_eq!(wrong.permissions, Permissions::default());
    assert_eq!(scalar.permissions, Permissions::default());
    let diagnostics = catalog.diagnostics.join("\n");
    for expected in [
        "Current: ignored malformed appPermissions.entitlements, appPermissions.privacy, permissions",
        "Map: ignored malformed appPermissions.privacy",
        "Wrong: ignored malformed appPermissions.entitlements, appPermissions.privacy, permissions",
        "Scalar: ignored malformed appPermissions",
    ] {
        assert!(diagnostics.contains(expected), "{diagnostics}");
    }
    assert!(!diagnostics.contains("Legacy: ignored"), "{diagnostics}");
}
#[test]
fn tint_colors_are_normalized_or_dropped() {
    let tint = |value: serde_json::Value| {
        catalog(
            json!({"name":"T","apps":[{"name":"App","bundleIdentifier":"test.app","version":"1",
            "downloadURL":"/a","tintColor":value,"category":"utilities"}]}),
        )
        .apps
        .remove(0)
    };
    for (input, expected) in [
        (json!("FF0000"), Some("#FF0000")),
        (json!("#0a84ff"), Some("#0A84FF")),
        (json!(" #f80 "), Some("#FF8800")),
        (json!("##FF0000"), None),
        (json!("red"), None),
        (json!("FF00001"), None),
        (json!("#GG0000"), None),
        (json!(16711680), None),
    ] {
        assert_eq!(
            tint(input.clone()).tint_color.as_deref(),
            expected,
            "{input}"
        );
    }
    assert_eq!(tint(json!(null)).category.as_deref(), Some("utilities"));
}
#[test]
fn sources_round_trip_through_json_with_defaults() {
    let url = Url::parse(RAW).unwrap();
    let source = Source {
        key: source_key(&url),
        url: url.clone(),
        enabled: false,
        catalog: parse_catalog(include_bytes!("fixtures/youmod.json"), &url).unwrap(),
        etag: Some("\"v1\"".into()),
        last_modified: Some("Wed, 01 Jan 2025 00:00:00 GMT".into()),
        last_success: Some(chrono::Utc::now()),
        error: Some("HTTP 503".into()),
        link: None,
    };
    let json = serde_json::to_string(&source).unwrap();
    let back: Source = serde_json::from_str(&json).unwrap();
    assert_eq!(serde_json::to_string(&back).unwrap(), json);
    assert_eq!(back.catalog.apps[0].permissions.privacy.len(), 7);
    assert_eq!(back.catalog.apps[0].tint_color.as_deref(), Some("#FF0000"));
    let mut minimal: serde_json::Value = serde_json::from_str(&json).unwrap();
    for key in ["enabled", "etag", "last_modified", "last_success", "error"] {
        minimal.as_object_mut().unwrap().remove(key);
    }
    for key in ["tint_color", "category", "permissions"] {
        minimal["catalog"]["apps"][0]
            .as_object_mut()
            .unwrap()
            .remove(key);
    }
    let minimal: Source = serde_json::from_value(minimal).unwrap();
    assert!(minimal.enabled);
    assert!(minimal.etag.is_none() && minimal.error.is_none());
    assert_eq!(minimal.catalog.apps[0].permissions, Permissions::default());
}
#[test]
fn public_policy_rejects_plain_http_and_non_public_ip_literals() {
    for url in [
        "http://example.com/feed",
        "https://127.0.0.1/feed",
        "https://2130706433/feed",
        "https://10.1.2.3/feed",
        "https://172.16.0.1/feed",
        "https://192.168.1.10/feed",
        "https://169.254.169.254/latest",
        "https://100.100.100.100/feed",
        "https://0.0.0.0/feed",
        "https://224.0.0.1/feed",
        "https://[::1]/feed",
        "https://[::]/feed",
        "https://[fd7a:115c:a1e0::1]/feed",
        "https://[fe80::1]/feed",
        "https://[::ffff:192.168.0.1]/feed",
        "https://[2002:c0a8:0001::1]/feed",
        "https://[64:ff9b::7f00:1]/feed",
        "https://[2001:db8::1]/feed",
    ] {
        assert!(public_url(&Url::parse(url).unwrap()).is_err(), "{url}");
    }
    for url in [
        "https://example.com/feed",
        "https://localhost/feed", // names are checked after DNS resolution
        "https://93.184.215.14/feed",
        "https://[2606:4700:4700::1111]/feed",
        "https://[::ffff:8.8.8.8]/feed",
        "https://[64:ff9b::808:808]/feed",
    ] {
        assert!(public_url(&Url::parse(url).unwrap()).is_ok(), "{url}");
    }
}
