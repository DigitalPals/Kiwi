use kiwi_device::*;
use serde_json::Value;

fn fixture(name: &str) -> Vec<u8> {
    let path = format!("{}/tests/fixtures/{name}", env!("CARGO_MANIFEST_DIR"));
    std::fs::read(&path).unwrap_or_else(|e| panic!("{path}: {e}"))
}

fn find<'a>(devices: &'a [CoreDevice], id: &str) -> &'a CoreDevice {
    devices.iter().find(|d| d.identifier == id).unwrap()
}

#[test]
fn simulators_from_real_output() {
    let devices = parse_devices(&fixture("devicectl_list_devices_simulators.json")).unwrap();
    assert_eq!(devices.len(), 3);
    let d = find(&devices, "E67F3739-DA98-4C02-B8B6-0D6FB192A2A7");
    assert_eq!(d.name, "iPhone 18 Pro");
    assert_eq!(
        d.udid.as_deref(),
        Some("E67F3739-DA98-4C02-B8B6-0D6FB192A2A7")
    );
    assert_eq!(d.model.as_deref(), Some("iPhone 18 Pro"));
    assert_eq!(d.product_type.as_deref(), Some("iPhone19,2"));
    assert_eq!(d.platform.as_deref(), Some("iOS"));
    assert_eq!(d.os_version.as_deref(), Some("27.0"));
    assert!(!d.physical);
    assert_eq!(d.pairing_state.as_deref(), Some("paired"));
    assert_eq!(d.tunnel_state.as_deref(), Some("connected"));
    assert_eq!(d.transport.as_deref(), Some("sameMachine"));
    assert_eq!(
        d.last_connection.unwrap().to_rfc3339(),
        "2026-10-29T06:15:38+00:00"
    );
    assert!(devices.iter().all(|d| !d.physical && d.reachable()));
    let duo = find(&devices, "8D2E9807-AA6F-4706-A6AA-B6EF5DE2C1BF");
    assert_eq!(duo.os_version.as_deref(), Some("27.1"));
    assert_eq!(duo.product_type.as_deref(), Some("iPhone19,4"));
}

#[test]
fn new_and_deprecated_fields_agree() {
    let full = parse_devices(&fixture("devicectl_list_devices_simulators.json")).unwrap();
    let omitted = parse_devices(&fixture("devicectl_list_devices_omit_deprecated.json")).unwrap();
    assert_eq!(full, omitted);

    // Same output with only the deprecated dictionaries.
    let mut json: Value =
        serde_json::from_slice(&fixture("devicectl_list_devices_simulators.json")).unwrap();
    for device in json["result"]["devices"].as_array_mut().unwrap() {
        device.as_object_mut().unwrap().remove("properties");
    }
    let deprecated = parse_devices(&serde_json::to_vec(&json).unwrap()).unwrap();
    assert_eq!(full, deprecated);
}

#[test]
fn physical_devices() {
    let devices = parse_devices(&fixture("devicectl_list_devices_physical.json")).unwrap();
    assert_eq!(devices.len(), 3);

    let iphone = find(&devices, "A1B2C3D4-E5F6-4789-9ABC-DEF012345678");
    assert!(iphone.physical && iphone.paired() && iphone.reachable());
    assert_eq!(iphone.name, "Alex\u{2019}s iPhone");
    assert_eq!(iphone.udid.as_deref(), Some("00008150-000A1B2C3D4E001E"));
    assert_eq!(iphone.product_type.as_deref(), Some("iPhone19,4"));
    assert_eq!(iphone.os_version.as_deref(), Some("27.0.1"));
    assert_eq!(iphone.transport.as_deref(), Some("localNetwork"));
    assert_eq!(iphone.tunnel_state.as_deref(), Some("connected"));
    assert_eq!(
        iphone.last_connection.unwrap().to_rfc3339(),
        "2026-10-29T08:05:12+00:00"
    );

    // Deprecated fields only, paired but out of reach.
    let ipad = find(&devices, "B2C3D4E5-F6A7-4890-ABCD-EF0123456789");
    assert!(ipad.physical && ipad.paired() && !ipad.reachable());
    assert_eq!(ipad.name, "Kitchen iPad");
    assert_eq!(ipad.model.as_deref(), Some("iPad Air 13-inch (M3)"));
    assert_eq!(ipad.os_version, None);
    assert_eq!(ipad.transport, None);
    assert_eq!(ipad.tunnel_state.as_deref(), Some("unavailable"));
    assert_eq!(
        ipad.last_connection.unwrap().to_rfc3339(),
        "2026-09-30T18:02:11.123+00:00"
    );

    // `properties` only, not paired.
    let unpaired = find(&devices, "C3D4E5F6-A7B8-4901-BCDE-F01234567890");
    assert!(unpaired.physical && !unpaired.paired() && !unpaired.reachable());
    assert_eq!(unpaired.os_version.as_deref(), Some("26.4"));
    assert_eq!(unpaired.transport.as_deref(), Some("wired"));
    assert_eq!(unpaired.last_connection, None);
}

#[test]
fn tolerant_device_parsing() {
    let json = serde_json::json!({
        "info": {"outcome": "success", "jsonVersion": 6},
        "result": {"devices": [
            {"identifier": "X", "visibilityClass": "simulators"},
            {"identifier": "Y", "properties": {"hardware": {"reality": "physical"}}},
            {"identifier": "Z"},
            {"name": "no identifier"},
        ]},
    });
    let devices = parse_devices(&serde_json::to_vec(&json).unwrap()).unwrap();
    assert_eq!(devices.len(), 3);
    assert_eq!(devices[0].name, "X");
    assert!(!devices[0].physical);
    assert!(devices[1].physical && devices[2].physical);
    assert!(parse_devices(b"{}").is_err());
    assert!(parse_devices(b"not json").is_err());
}

#[test]
fn simulator_apps() {
    let apps = parse_apps(&fixture("devicectl_apps_simulator.json")).unwrap();
    assert_eq!(apps.len(), 2);
    let sample = apps
        .iter()
        .find(|a| a.bundle_id == "com.example.sample")
        .unwrap();
    assert_eq!(sample.name.as_deref(), Some("Sample"));
    assert_eq!(sample.version.as_deref(), Some("3.1.0"));
    assert_eq!(sample.build.as_deref(), Some("3001000"));
    assert_eq!(sample.removable, Some(true));
    let path = sample.path.as_deref().unwrap();
    assert!(path.starts_with("/Users/alex/Library/Developer/CoreSimulator/Devices/"));
    assert!(path.ends_with("/Sample.app"));
}

#[test]
fn physical_apps() {
    let apps = parse_apps(&fixture("devicectl_apps_physical.json")).unwrap();
    assert_eq!(apps.len(), 3);
    assert_eq!(apps[0].bundle_id, "com.google.ios.youtube.A1B2C3D4E5");
    assert_eq!(apps[0].version.as_deref(), Some("21.43.5"));
    assert_eq!(
        apps[1].path.as_deref(),
        Some(
            "/private/var/containers/Bundle/Application/1E2F3A4B-5C6D-4E7F-9A0B-1C2D3E4F5A6B/My App.app"
        )
    );
    assert_eq!(
        (apps[1].version.as_deref(), apps[1].build.as_deref()),
        (None, None)
    );
    assert_eq!(
        apps[2],
        DeviceApp {
            bundle_id: "com.example.minimal".into(),
            name: None,
            version: None,
            build: None,
            removable: None,
            path: None,
        }
    );
}

#[test]
fn devicectl_failure_output() {
    let err = parse_apps(&fixture("devicectl_apps_device_not_found.json")).unwrap_err();
    match err {
        Error::Failed { program, message } => {
            assert_eq!(program, "devicectl");
            assert_eq!(
                message,
                "The specified device was not found. (Name: 00000000-0000-0000-0000-000000000000)"
            );
        }
        other => panic!("{other:?}"),
    }
}

#[test]
fn whois_iphone() {
    let peer = parse_whois(&fixture("tailscale_whois_iphone.json")).unwrap();
    assert_eq!(
        peer,
        TailscalePeer {
            stable_id: "nPHONE1CNTRL".into(),
            name: "iphone194".into(),
            dns_name: "iphone194.example.ts.net".into(),
            os: Some("iOS".into()),
            user_login: Some("user@example.com".into()),
            user_name: Some("Alex Morgan".into()),
        }
    );
    let minimal =
        parse_whois(br#"{"Node":{"StableID":"nX","Name":"mac-studio.ts.net."}}"#).unwrap();
    assert_eq!(minimal.name, "mac-studio");
    assert_eq!(minimal.dns_name, "mac-studio.ts.net");
    assert_eq!((minimal.os, minimal.user_login), (None, None));
    assert!(parse_whois(b"{}").is_err());
}

#[test]
fn status_owner() {
    let owner = parse_owner(&fixture("tailscale_status.json")).unwrap();
    assert_eq!(
        owner,
        Some(TailscaleUser {
            login: "alex@example.com".into(),
            name: Some("Alex Morgan".into()),
        })
    );
    // A tagged machine belongs to no user; a missing user is unknown.
    let mut status: Value = serde_json::from_slice(&fixture("tailscale_status.json")).unwrap();
    status["Self"]["Tags"] = serde_json::json!(["tag:server"]);
    assert_eq!(parse_owner(status.to_string().as_bytes()).unwrap(), None);
    status["Self"]["Tags"] = Value::Null;
    status["Self"]["UserID"] = serde_json::json!(1);
    assert_eq!(parse_owner(status.to_string().as_bytes()).unwrap(), None);
    assert_eq!(parse_owner(b"{}").unwrap(), None);
    assert!(parse_owner(b"[").is_err());
}

#[test]
fn status_dns_name() {
    let status = fixture("tailscale_status.json");
    assert_eq!(
        parse_dns_name(&status).unwrap().as_deref(),
        Some("mac-studio.example.ts.net")
    );
    // Before MagicDNS names the machine.
    let mut status: Value = serde_json::from_slice(&status).unwrap();
    status["Self"]["DNSName"] = serde_json::json!("");
    assert_eq!(parse_dns_name(status.to_string().as_bytes()).unwrap(), None);
    assert_eq!(parse_dns_name(b"{}").unwrap(), None);
    assert!(parse_dns_name(b"[").is_err());
}

#[cfg(unix)]
#[tokio::test]
async fn whois_command() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let fixture_path = format!(
        "{}/tests/fixtures/tailscale_whois_iphone.json",
        env!("CARGO_MANIFEST_DIR")
    );
    let bin = dir.path().join("tailscale");
    let script = format!(
        "#!/bin/sh\n[ \"$1 $2\" = \"whois --json\" ] || exit 9\ncase \"$3\" in\n\
         100.64.0.7) cat '{fixture_path}' ;;\n\
         fd7a:115c:a1e0::1) echo 'no match for IP:port' >&2; exit 1 ;;\n\
         100.64.0.9) echo 'peer not found' >&2; exit 1 ;;\n\
         *) echo 'failed to connect to local tailscaled; it doesn'\"'\"'t appear to be running' >&2; exit 1 ;;\n\
         esac\n"
    );
    std::fs::write(&bin, script).unwrap();
    std::fs::set_permissions(&bin, std::fs::Permissions::from_mode(0o755)).unwrap();

    let peer = tailscale_whois(&bin, "100.64.0.7".parse().unwrap())
        .await
        .unwrap();
    assert_eq!(peer.unwrap().stable_id, "nPHONE1CNTRL");
    let mapped = tailscale_whois(&bin, "::ffff:100.64.0.7".parse().unwrap()).await;
    assert!(mapped.unwrap().is_some());
    assert_eq!(
        tailscale_whois(&bin, "100.64.0.9".parse().unwrap())
            .await
            .unwrap(),
        None
    );
    assert_eq!(
        tailscale_whois(&bin, "fd7a:115c:a1e0::1".parse().unwrap())
            .await
            .unwrap(),
        None
    );
    let err = tailscale_whois(&bin, "10.0.0.1".parse().unwrap())
        .await
        .unwrap_err();
    assert!(
        matches!(&err, Error::Failed { program, message } if program == "tailscale" && message.contains("tailscaled")),
        "{err}"
    );
    let missing = dir.path().join("missing");
    assert!(matches!(
        tailscale_whois(&missing, "100.64.0.9".parse().unwrap()).await,
        Err(Error::Spawn { .. })
    ));
}

/// Answers like tailscaled's LocalAPI, one request per connection.
#[cfg(unix)]
async fn fake_tailscaled(listener: tokio::net::UnixListener) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    loop {
        let (mut conn, _) = listener.accept().await.unwrap();
        tokio::spawn(async move {
            let mut head = Vec::new();
            let mut chunk = [0; 1024];
            while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                match conn.read(&mut chunk).await {
                    Ok(0) | Err(_) => return,
                    Ok(n) => head.extend_from_slice(&chunk[..n]),
                }
            }
            let head = String::from_utf8_lossy(&head).to_ascii_lowercase();
            let target = head.split(' ').nth(1).unwrap_or_default();
            let (status, body) = match target {
                _ if !head.contains("\r\nhost: local-tailscaled.sock\r\n") => {
                    ("403 Forbidden", b"invalid localapi request\n".to_vec())
                }
                "/localapi/v0/status?peers=false" => ("200 OK", fixture("tailscale_status.json")),
                "/localapi/v0/whois?addr=100.64.0.7" => {
                    ("200 OK", fixture("tailscale_whois_iphone.json"))
                }
                "/localapi/v0/whois?addr=10.0.0.1" => {
                    ("403 Forbidden", b"whois access denied\n".to_vec())
                }
                _ => ("404 Not Found", b"no match for IP:port\n".to_vec()),
            };
            let head = format!(
                "HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = conn.write_all(head.as_bytes()).await;
            let _ = conn.write_all(&body).await;
        });
    }
}

#[cfg(unix)]
#[tokio::test]
async fn localapi() {
    let dir = tempfile::tempdir().unwrap();
    let socket = dir.path().join("tailscaled.socket");
    tokio::spawn(fake_tailscaled(
        tokio::net::UnixListener::bind(&socket).unwrap(),
    ));
    // The CLI is never run while the socket is there.
    let ts = Tailscale::new(&socket, dir.path().join("missing")).unwrap();
    assert!(ts.uses_localapi());

    let peer = ts.whois("100.64.0.7".parse().unwrap()).await.unwrap();
    assert_eq!(peer.unwrap().stable_id, "nPHONE1CNTRL");
    let mapped = ts.whois("::ffff:100.64.0.7".parse().unwrap()).await;
    assert!(mapped.unwrap().is_some());
    assert_eq!(ts.whois("100.64.0.9".parse().unwrap()).await.unwrap(), None);
    assert_eq!(
        ts.whois("fd7a:115c:a1e0::1".parse().unwrap())
            .await
            .unwrap(),
        None
    );
    let err = ts.whois("10.0.0.1".parse().unwrap()).await.unwrap_err();
    assert!(
        matches!(&err, Error::Failed { program, message } if program == "tailscaled" && message == "HTTP 403: whois access denied"),
        "{err}"
    );
    assert_eq!(
        ts.owner().await.unwrap(),
        Some(TailscaleUser {
            login: "alex@example.com".into(),
            name: Some("Alex Morgan".into()),
        })
    );

    // A socket left behind by a stopped tailscaled.
    let stale = dir.path().join("stale.socket");
    drop(std::os::unix::net::UnixListener::bind(&stale).unwrap());
    let ts = Tailscale::new(&stale, dir.path().join("missing")).unwrap();
    let err = ts.whois("100.64.0.9".parse().unwrap()).await.unwrap_err();
    assert!(
        matches!(&err, Error::Failed { program, message } if program == "tailscaled" && message.starts_with(&stale.display().to_string())),
        "{err}"
    );
    assert!(ts.owner().await.is_err());

    // Without a socket the CLI is used.
    let ts = Tailscale::new(dir.path().join("none.socket"), dir.path().join("missing")).unwrap();
    assert!(!ts.uses_localapi());
    assert!(matches!(
        ts.whois("100.64.0.9".parse().unwrap()).await,
        Err(Error::Spawn { .. })
    ));
    assert!(matches!(ts.owner().await, Err(Error::Spawn { .. })));
}

#[cfg(not(target_os = "macos"))]
#[tokio::test]
async fn devicectl_unsupported_off_macos() {
    assert!(!devicectl_available().await);
    assert!(matches!(list_devices().await, Err(Error::Unsupported)));
    assert!(matches!(list_apps("X").await, Err(Error::Unsupported)));
    assert!(matches!(
        install_app("X", std::path::Path::new("/tmp/a.ipa")).await,
        Err(Error::Unsupported)
    ));
}

fn ua(s: &str) -> Option<UaInfo> {
    parse_user_agent(s)
}

fn info(
    os_version: Option<String>,
    model: Option<String>,
    build: Option<String>,
    source: UaSource,
) -> Option<UaInfo> {
    Some(UaInfo {
        os_version,
        model,
        build,
        source,
    })
}

fn some(v: &str) -> Option<String> {
    Some(v.to_owned())
}

#[test]
fn safari_user_agents() {
    // iOS 26+ freezes the OS token at 18_6; Version/ carries the real major.minor.
    assert_eq!(
        ua(
            "Mozilla/5.0 (iPhone; CPU iPhone OS 18_6 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/27.0 Mobile/15E148 Safari/604.1"
        ),
        info(some("27.0"), None, None, UaSource::Safari)
    );
    assert_eq!(
        ua(
            "Mozilla/5.0 (iPhone; CPU iPhone OS 18_6 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/26.1 Mobile/15E148 Safari/604.1"
        ),
        info(some("26.1"), None, None, UaSource::Safari)
    );
    assert_eq!(
        ua(
            "Mozilla/5.0 (iPad; CPU OS 18_6 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/27.1 Mobile/15E148 Safari/604.1"
        ),
        info(some("27.1"), None, None, UaSource::Safari)
    );
    // Before the freeze the OS token is exact.
    assert_eq!(
        ua(
            "Mozilla/5.0 (iPhone; CPU iPhone OS 17_5_1 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/17.5 Mobile/15E148 Safari/604.1"
        ),
        info(some("17.5.1"), None, None, UaSource::Safari)
    );
    assert_eq!(
        ua(
            "Mozilla/5.0 (iPhone; CPU iPhone OS 18_6 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.6 Mobile/15E148 Safari/604.1"
        ),
        info(some("18.6"), None, None, UaSource::Safari)
    );
    // Home Screen web app / WKWebView without Version/: only an unfrozen token is usable.
    assert_eq!(
        ua(
            "Mozilla/5.0 (iPhone; CPU iPhone OS 17_4 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Mobile/15E148"
        ),
        info(some("17.4"), None, None, UaSource::Safari)
    );
    assert_eq!(
        ua(
            "Mozilla/5.0 (iPhone; CPU iPhone OS 18_6 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) Mobile/15E148"
        ),
        None
    );
    assert_eq!(
        ua(
            "Mozilla/5.0 (iPhone; CPU iPhone OS 18_6 like Mac OS X) AppleWebKit/605.1.15 (KHTML, like Gecko) CriOS/141.0.7390.41 Mobile/15E148 Safari/604.1"
        ),
        None
    );
    // Not iOS (desktop Safari, iPad desktop mode, Android).
    assert_eq!(
        ua(
            "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/605.1.15 (KHTML, like Gecko) Version/27.0 Safari/605.1.15"
        ),
        None
    );
    assert_eq!(
        ua(
            "Mozilla/5.0 (Linux; Android 15) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0 Mobile Safari/537.36"
        ),
        None
    );
}

#[test]
fn installer_user_agents() {
    let exact = info(
        some("27.0.1"),
        some("iPhone19,4"),
        some("24A434"),
        UaSource::Installer,
    );
    assert_eq!(
        ua("itunesstored/1.0 iOS/27.0.1 model/iPhone19,4 hwp/t8150 build/24A434 (6; dt:332) AMS/1"),
        exact
    );
    assert_eq!(
        ua(
            "com.apple.appstored/1.0 iOS/27.0.1 model/iPhone19,4 hwp/t8150 build/24A434 (6; dt:332) AMS/1"
        ),
        exact
    );
    assert_eq!(
        ua("appstored/1 iOS/27.0.1 model/iPhone19,4 hwp/t8150 build/24A434 (6; dt:332)"),
        exact
    );
    assert_eq!(
        ua("com.apple.itunesstored/1.0 iOS/9.3 model/iPhone7,2 hwp/t7000 build/13E234 (6; dt:107)"),
        info(
            some("9.3"),
            some("iPhone7,2"),
            some("13E234"),
            UaSource::Installer
        )
    );
    assert_eq!(
        ua("AppStore/3.0 iPadOS/26.2 model/iPad16,3 hwp/t8132 build/23C55 (5; dt:340) AMS/1"),
        info(
            some("26.2"),
            some("iPad16,3"),
            some("23C55"),
            UaSource::Installer
        )
    );
    // Tolerant of order, missing fields and comment-style versions.
    assert_eq!(
        ua("installd/1.0 model/iPhone19,4 iOS/27.0"),
        info(some("27.0"), some("iPhone19,4"), None, UaSource::Installer)
    );
    assert_eq!(
        ua("Kiwi/1 (iPhone; iOS 27.0.1; Scale/3.00)"),
        info(some("27.0.1"), None, None, UaSource::Installer)
    );
    // Junk values are ignored.
    assert_eq!(ua("itunesstored/1.0 iOS/x.y model/<script> build/!!"), None);
    assert_eq!(
        ua("itunesstored/1.0 CFNetwork/3860.100.1 Darwin/26.0.0"),
        None
    );
    assert_eq!(ua("curl/8.10.1"), None);
    assert_eq!(ua(""), None);
    assert_eq!(UaSource::Installer.as_str(), "installer");
    assert_eq!(UaSource::Safari.as_str(), "safari");
}
