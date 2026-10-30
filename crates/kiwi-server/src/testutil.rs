//! Fixtures for unit tests.

use chrono::Duration;
use kiwi_signer::{Identity, Profile};
use kiwi_source::{App, Permissions, Release, Url};

use crate::db::{BuildRow, BuildStatus, InstallRow, SourceRow, Time};

pub const TEAM: &str = "A1B2C3D4E5";
pub const SHA1: &str = "0123456789ABCDEF0123456789ABCDEF01234567";

pub fn identity(expires_at: Time) -> Identity {
    Identity {
        sha1: SHA1.into(),
        name: "Apple Development: Test (Z9Y8X7W6V5)".into(),
        team_id: Some(TEAM.into()),
        expires_at: Some(expires_at),
    }
}

pub fn profile(uuid: &str, app_id_pattern: &str, expires_at: Time) -> Profile {
    let mut entitlements = plist::Dictionary::new();
    for key in [
        "application-identifier",
        "com.apple.developer.team-identifier",
        "get-task-allow",
        "keychain-access-groups",
    ] {
        entitlements.insert(key.into(), plist::Value::Boolean(true));
    }
    Profile {
        uuid: uuid.into(),
        name: format!("Profile {uuid}"),
        path: format!("/profiles/{uuid}.mobileprovision").into(),
        team_id: TEAM.into(),
        app_id_pattern: app_id_pattern.into(),
        created_at: expires_at - Duration::days(365),
        expires_at,
        devices: vec!["00008150-0000000000000001".into()],
        entitlements,
        certificate_sha1s: vec![SHA1.into()],
        platforms: vec!["iOS".into()],
        development: true,
    }
}

pub fn release(version: &str, min_os: Option<&str>) -> Release {
    Release {
        version: version.into(),
        build: None,
        revision: None,
        date: None,
        notes: None,
        download_url: Url::parse(&format!("https://example.com/app-{version}.ipa")).unwrap(),
        size: Some(1000),
        sha256: None,
        min_os: min_os.map(Into::into),
        max_os: None,
        platforms: Vec::new(),
    }
}

/// `com.example.app` with releases (version, min OS), newest first.
pub fn app(releases: &[(&str, Option<&str>)]) -> App {
    App {
        name: "Example".into(),
        bundle_id: "com.example.app".into(),
        developer: None,
        subtitle: None,
        description: None,
        icon_url: None,
        screenshots: Vec::new(),
        tint_color: None,
        category: None,
        permissions: Permissions::default(),
        releases: releases.iter().map(|(v, os)| release(v, *os)).collect(),
        unavailable: None,
    }
}

pub fn build_row(id: &str, status: BuildStatus, now: Time) -> BuildRow {
    let release = release("1.0", None);
    BuildRow {
        id: id.into(),
        source_id: "src".into(),
        bundle_id: "com.example.app".into(),
        app_name: "Example".into(),
        icon_url: None,
        version: release.version.clone(),
        build: None,
        release,
        artifact_sha256: None,
        signed_bundle_id: format!("com.example.app.{TEAM}"),
        url_scheme: Some("kiwi-0123456789ab".into()),
        identity_sha1: Some(SHA1.into()),
        profile_uuids: vec!["wild".into()],
        signing_key: None,
        expires_at: Some(now + Duration::days(365)),
        ipa_path: None,
        size: Some(10),
        status,
        step: None,
        progress: None,
        error: None,
        retryable: false,
        dropped_entitlements: Vec::new(),
        created_at: now,
        finished_at: None,
        app_version: None,
    }
}

pub fn build_for(release: &Release, status: BuildStatus, now: Time) -> BuildRow {
    BuildRow {
        id: format!("build-{}-{}", release.version, status.as_str()),
        version: release.version.clone(),
        build: release.build.clone(),
        release: release.clone(),
        ..build_row("b", status, now)
    }
}

pub fn install_of(release: &Release, origin: &str, now: Time) -> InstallRow {
    InstallRow {
        device_id: "ts:test".into(),
        signed_bundle_id: format!("com.example.app.{TEAM}"),
        source_id: "src".into(),
        bundle_id: "com.example.app".into(),
        name: "Example".into(),
        icon_url: None,
        developer: None,
        build_id: None,
        version: release.version.clone(),
        build: release.build.clone(),
        release: Some(release.clone()),
        url_scheme: Some("kiwi-0123456789ab".into()),
        expires_at: Some(now + Duration::days(365)),
        origin: origin.into(),
        verified: false,
        installed_at: now,
        last_seen_at: None,
        removed_at: None,
    }
}

pub fn source_row(id: &str, apps: Vec<App>) -> SourceRow {
    let url = Url::parse("https://example.com/source.json").unwrap();
    let now = chrono::Utc::now();
    SourceRow {
        id: id.into(),
        url: url.to_string(),
        enabled: true,
        position: 0,
        added_at: now,
        last_attempt_at: None,
        browse: crate::db::BrowseMode::All,
        picked_at: None,
        source: kiwi_source::Source {
            key: id.into(),
            url,
            enabled: true,
            catalog: kiwi_source::Catalog {
                name: "Test Source".into(),
                identifier: None,
                source_url: None,
                subtitle: None,
                description: None,
                icon_url: None,
                apps,
                diagnostics: Vec::new(),
            },
            etag: None,
            last_modified: None,
            last_success: Some(now),
            error: None,
        },
    }
}
