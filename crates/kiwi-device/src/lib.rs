//! Device discovery for Kiwi: `xcrun devicectl` (macOS only), Tailscale (tailscaled's
//! LocalAPI or the `tailscale` CLI) and User-Agent parsing.

mod devicectl;
mod process;
mod tailscale;
mod ua;

use std::time::Duration;

pub use devicectl::{
    CoreDevice, DeviceApp, devicectl_available, install_app, list_apps, list_devices, parse_apps,
    parse_devices,
};
pub use tailscale::{
    TAILSCALED_SOCKET, Tailscale, TailscalePeer, TailscaleUser, parse_dns_name, parse_owner,
    parse_whois, tailscale_owner, tailscale_whois,
};
pub use ua::{UaInfo, UaSource, parse_user_agent};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("devicectl is only available on macOS")]
    Unsupported,
    #[error("could not run {program}: {source}")]
    Spawn {
        program: String,
        source: std::io::Error,
    },
    #[error("{program} timed out after {}s", .after.as_secs())]
    Timeout { program: String, after: Duration },
    #[error("{program} failed: {message}")]
    Failed { program: String, message: String },
    #[error("unexpected {program} output: {message}")]
    Parse { program: String, message: String },
}

pub type Result<T, E = Error> = std::result::Result<T, E>;
