use std::net::IpAddr;
use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use crate::{Error, Result, process};

const WHOIS_TIMEOUT: Duration = Duration::from_secs(10);

/// Where the open-source tailscaled listens for its LocalAPI: Homebrew's on macOS, the
/// distribution packages' on Linux.
#[cfg(target_os = "macos")]
pub const TAILSCALED_SOCKET: &str = "/var/run/tailscaled.socket";
#[cfg(not(target_os = "macos"))]
pub const TAILSCALED_SOCKET: &str = "/var/run/tailscale/tailscaled.socket";

/// tailscaled refuses LocalAPI requests for other host names (DNS rebinding).
const LOCALAPI: &str = "http://local-tailscaled.sock/localapi/v0";

/// Tailscale on this machine. Kiwi asks tailscaled directly through its LocalAPI socket, and
/// runs the `tailscale` CLI only where there is no socket (the Mac App Store and standalone
/// Tailscale apps). The socket is looked for on every call, so a tailscaled installed or
/// removed later is picked up.
pub struct Tailscale {
    socket: PathBuf,
    bin: PathBuf,
    client: reqwest::Client,
}

impl Tailscale {
    pub fn new(socket: impl Into<PathBuf>, bin: impl Into<PathBuf>) -> Result<Self> {
        let socket = socket.into();
        let client = reqwest::Client::builder()
            .unix_socket(socket.as_path())
            .timeout(WHOIS_TIMEOUT)
            .build()
            .map_err(|e| Error::Failed {
                program: "tailscaled".into(),
                message: e.to_string(),
            })?;
        Ok(Self {
            socket,
            bin: bin.into(),
            client,
        })
    }

    pub fn socket(&self) -> &Path {
        &self.socket
    }

    pub fn bin(&self) -> &Path {
        &self.bin
    }

    /// Whether calls go to tailscaled's socket rather than the CLI.
    pub fn uses_localapi(&self) -> bool {
        self.socket.exists()
    }

    /// The tailnet node at `ip`; `None` when it is not a peer.
    pub async fn whois(&self, ip: IpAddr) -> Result<Option<TailscalePeer>> {
        if !self.uses_localapi() {
            return tailscale_whois(&self.bin, ip).await;
        }
        // `::ffff:a.b.c.d` (dual-stack sockets) is looked up as plain IPv4.
        let ip = ip.to_canonical();
        match self.get(&format!("whois?addr={ip}")).await? {
            (200, body) => parse_whois(&body).map(Some),
            // tailscaled's own client also reads any 404 as "peer not found".
            (404, _) => Ok(None),
            (status, body) => Err(self.status_error(status, &body)),
        }
    }

    /// The user who owns this machine; `None` for a tagged machine.
    pub async fn owner(&self) -> Result<Option<TailscaleUser>> {
        if !self.uses_localapi() {
            return tailscale_owner(&self.bin).await;
        }
        parse_owner(&self.status().await?)
    }

    /// This machine's MagicDNS name without the trailing dot (`mac.tailnet.ts.net`);
    /// `None` before it has one.
    pub async fn dns_name(&self) -> Result<Option<String>> {
        if !self.uses_localapi() {
            return parse_dns_name(&cli_status(&self.bin).await?);
        }
        parse_dns_name(&self.status().await?)
    }

    /// `tailscale status --json` from the LocalAPI. Without peers the status still lists
    /// this machine and its own user.
    async fn status(&self) -> Result<Vec<u8>> {
        match self.get("status?peers=false").await? {
            (200, body) => Ok(body),
            (status, body) => Err(self.status_error(status, &body)),
        }
    }

    async fn get(&self, path: &str) -> Result<(u16, Vec<u8>)> {
        let response = self
            .client
            .get(format!("{LOCALAPI}/{path}"))
            .send()
            .await
            .map_err(|e| self.request_error(e))?;
        let status = response.status().as_u16();
        let body = response.bytes().await.map_err(|e| self.request_error(e))?;
        Ok((status, body.to_vec()))
    }

    fn request_error(&self, e: reqwest::Error) -> Error {
        if e.is_timeout() {
            return Error::Timeout {
                program: "tailscaled".into(),
                after: WHOIS_TIMEOUT,
            };
        }
        // reqwest's own message names the URL; the cause (e.g. "Connection refused") helps.
        let mut cause: &dyn std::error::Error = &e;
        while let Some(source) = cause.source() {
            cause = source;
        }
        Error::Failed {
            program: "tailscaled".into(),
            message: format!("{}: {cause}", self.socket.display()),
        }
    }

    fn status_error(&self, status: u16, body: &[u8]) -> Error {
        let body = String::from_utf8_lossy(body);
        let first = body.trim().lines().next().unwrap_or_default().trim();
        Error::Failed {
            program: "tailscaled".into(),
            message: format!(
                "HTTP {status}: {}",
                first.chars().take(500).collect::<String>()
            ),
        }
    }
}

/// A tailnet node as reported by `tailscale whois`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TailscalePeer {
    /// `Node.StableID`, e.g. "nPHONE1CNTRL".
    pub stable_id: String,
    /// `Node.ComputedName`, e.g. "iphone194".
    pub name: String,
    /// `Node.Name` without the trailing dot, e.g. "iphone194.example.ts.net".
    pub dns_name: String,
    /// `Node.Hostinfo.OS`, e.g. "iOS".
    pub os: Option<String>,
    pub user_login: Option<String>,
    pub user_name: Option<String>,
}

/// Runs `<bin> whois --json <ip>`; `None` when the address is not a tailnet peer.
/// Works on any platform with a Tailscale CLI.
pub async fn tailscale_whois(bin: &Path, ip: IpAddr) -> Result<Option<TailscalePeer>> {
    // `::ffff:a.b.c.d` (dual-stack sockets) is looked up as plain IPv4.
    let ip = ip.to_canonical();
    let args = ["whois".to_owned(), "--json".to_owned(), ip.to_string()];
    let output = process::run(bin, &args, WHOIS_TIMEOUT).await?;
    if output.status.success() {
        return parse_whois(&output.stdout).map(Some);
    }
    let text = [&output.stderr[..], &output.stdout[..]].concat();
    let text = String::from_utf8_lossy(&text).to_ascii_lowercase();
    if text.contains("peer not found") || text.contains("no match for") {
        return Ok(None);
    }
    Err(Error::Failed {
        program: process::name(bin),
        message: process::failure_message(&output),
    })
}

/// The Tailscale user who owns this machine (`tailscale status`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TailscaleUser {
    /// `LoginName`, e.g. "alex@example.com".
    pub login: String,
    /// `DisplayName`, e.g. "Alex Morgan".
    pub name: Option<String>,
}

/// Runs `<bin> status --json` and returns the user who owns this machine; `None` for a
/// tagged machine, which belongs to no user.
pub async fn tailscale_owner(bin: &Path) -> Result<Option<TailscaleUser>> {
    parse_owner(&cli_status(bin).await?)
}

/// `<bin> status --json`.
async fn cli_status(bin: &Path) -> Result<Vec<u8>> {
    let args = ["status".to_owned(), "--json".to_owned()];
    let output = process::run(bin, &args, WHOIS_TIMEOUT).await?;
    if !output.status.success() {
        return Err(Error::Failed {
            program: process::name(bin),
            message: process::failure_message(&output),
        });
    }
    Ok(output.stdout)
}

/// Parses `tailscale status --json` output: the `User` entry of `Self.UserID`.
pub fn parse_owner(json: &[u8]) -> Result<Option<TailscaleUser>> {
    let status: Status = serde_json::from_slice(json).map_err(|e| Error::Parse {
        program: "tailscale".into(),
        message: e.to_string(),
    })?;
    let Some(me) = status.self_node else {
        return Ok(None);
    };
    if !me.tags.unwrap_or_default().is_empty() {
        return Ok(None);
    }
    let user = me
        .user_id
        .and_then(|id| status.user.unwrap_or_default().remove(&id.to_string()));
    Ok(user.and_then(|u| {
        Some(TailscaleUser {
            login: non_empty(u.login_name)?,
            name: non_empty(u.display_name),
        })
    }))
}

/// Parses `tailscale status --json` output: `Self.DNSName` without the trailing dot.
pub fn parse_dns_name(json: &[u8]) -> Result<Option<String>> {
    let status: Status = serde_json::from_slice(json).map_err(|e| Error::Parse {
        program: "tailscale".into(),
        message: e.to_string(),
    })?;
    Ok(status
        .self_node
        .and_then(|me| non_empty(me.dns_name))
        .map(|name| name.trim_end_matches('.').to_ascii_lowercase()))
}

/// Parses `tailscale whois --json` output.
pub fn parse_whois(json: &[u8]) -> Result<TailscalePeer> {
    let whois: Whois = serde_json::from_slice(json).map_err(|e| Error::Parse {
        program: "tailscale".into(),
        message: e.to_string(),
    })?;
    let node = whois.node;
    let dns_name = node.name.trim_end_matches('.').to_owned();
    let name = non_empty(node.computed_name)
        .unwrap_or_else(|| dns_name.split('.').next().unwrap_or_default().to_owned());
    let user = whois.user_profile.unwrap_or_default();
    Ok(TailscalePeer {
        stable_id: node.stable_id,
        name,
        dns_name,
        os: non_empty(node.hostinfo.unwrap_or_default().os),
        user_login: non_empty(user.login_name),
        user_name: non_empty(user.display_name),
    })
}

fn non_empty(s: Option<String>) -> Option<String> {
    s.filter(|s| !s.trim().is_empty())
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Whois {
    node: Node,
    user_profile: Option<UserProfile>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Node {
    #[serde(rename = "StableID")]
    stable_id: String,
    #[serde(default)]
    name: String,
    computed_name: Option<String>,
    hostinfo: Option<Hostinfo>,
}

#[derive(Deserialize, Default)]
struct Hostinfo {
    #[serde(rename = "OS")]
    os: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Status {
    #[serde(rename = "Self")]
    self_node: Option<SelfNode>,
    user: Option<std::collections::HashMap<String, UserProfile>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct SelfNode {
    #[serde(rename = "UserID")]
    user_id: Option<i64>,
    tags: Option<Vec<String>>,
    #[serde(rename = "DNSName")]
    dns_name: Option<String>,
}

#[derive(Deserialize, Default)]
#[serde(rename_all = "PascalCase")]
struct UserProfile {
    login_name: Option<String>,
    display_name: Option<String>,
}
