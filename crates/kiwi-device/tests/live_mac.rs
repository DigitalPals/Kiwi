//! Real devicectl/Tailscale calls on a Mac (read-only), with KIWI_LIVE_IPHONE set to the
//! Tailscale IP of an iPhone on the tailnet:
//! `KIWI_LIVE_IPHONE=100.x.y.z cargo test -p kiwi-device --test live_mac -- --ignored --nocapture`
#![cfg(target_os = "macos")]

use std::{net::IpAddr, path::Path};

fn iphone() -> IpAddr {
    std::env::var("KIWI_LIVE_IPHONE")
        .expect("KIWI_LIVE_IPHONE: an iPhone's Tailscale IP")
        .parse()
        .expect("KIWI_LIVE_IPHONE must be an IP address")
}

#[tokio::test]
#[ignore = "needs Xcode, booted simulators and Tailscale"]
async fn live_devicectl_and_whois() {
    assert!(kiwi_device::devicectl_available().await);
    let devices = kiwi_device::list_devices().await.unwrap();
    for d in &devices {
        println!("{d:?}");
    }
    let sim = devices
        .iter()
        .find(|d| !d.physical && d.tunnel_state.as_deref() == Some("connected"))
        .expect("a booted simulator");
    let apps = kiwi_device::list_apps(&sim.identifier).await.unwrap();
    println!("{} apps on {}: {apps:#?}", apps.len(), sim.name);

    let err = kiwi_device::list_apps("00000000-0000-0000-0000-000000000000")
        .await
        .unwrap_err();
    println!("unknown device: {err}");

    let tailscale = Path::new("/opt/homebrew/bin/tailscale");
    let peer = kiwi_device::tailscale_whois(tailscale, iphone())
        .await
        .unwrap();
    println!("whois {}: {peer:?}", iphone());
    assert_eq!(peer.unwrap().os.as_deref(), Some("iOS"));
    let none = kiwi_device::tailscale_whois(tailscale, "100.64.99.99".parse().unwrap())
        .await
        .unwrap();
    assert_eq!(none, None);
}

/// `cargo test -p kiwi-device --test live_mac live_tailscale -- --ignored --nocapture`
#[tokio::test]
#[ignore = "needs Homebrew's tailscaled and the iPhone on the tailnet"]
async fn live_tailscale() {
    let cli = Path::new("/opt/homebrew/bin/tailscale");
    let ts = kiwi_device::Tailscale::new(kiwi_device::TAILSCALED_SOCKET, cli).unwrap();
    assert!(ts.uses_localapi());
    let iphone = iphone();
    let peer = ts.whois(iphone).await.unwrap();
    println!("LocalAPI whois {iphone}: {peer:?}");
    assert_eq!(
        peer,
        kiwi_device::tailscale_whois(cli, iphone).await.unwrap()
    );
    assert_eq!(peer.unwrap().os.as_deref(), Some("iOS"));
    assert_eq!(
        ts.whois("100.64.99.99".parse().unwrap()).await.unwrap(),
        None
    );
    let owner = ts.owner().await.unwrap();
    println!("LocalAPI owner: {owner:?}");
    assert!(owner.is_some());
    assert_eq!(owner, kiwi_device::tailscale_owner(cli).await.unwrap());
}
