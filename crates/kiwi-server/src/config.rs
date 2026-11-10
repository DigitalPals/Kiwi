use std::{net::SocketAddr, path::PathBuf};

use clap::Parser;

/// The `tailscale` CLI of a Homebrew install on macOS, of the distribution packages elsewhere.
#[cfg(target_os = "macos")]
pub const TAILSCALE_BIN: &str = "/opt/homebrew/bin/tailscale";
#[cfg(not(target_os = "macos"))]
pub const TAILSCALE_BIN: &str = "/usr/bin/tailscale";

/// The Python that runs Kiwi Builds' runners. Apple's `/usr/bin/python3` is too old, so on
/// macOS it is the Python 3.13 that the Mac setup installs with Homebrew.
#[cfg(target_os = "macos")]
pub const PYTHON: &str = "/opt/homebrew/bin/python3.13";
#[cfg(not(target_os = "macos"))]
pub const PYTHON: &str = "python3";

/// Kiwi: a private, self-hosted iOS app store.
#[derive(Debug, Parser)]
#[command(name = "kiwi", version)]
pub struct Args {
    /// Address to listen on (loopback; `tailscale serve` exposes it).
    #[arg(long, env = "KIWI_LISTEN", default_value = "127.0.0.1:8742")]
    pub listen: SocketAddr,
    /// State directory [default: ~/Library/Application Support/Kiwi on macOS, $XDG_DATA_HOME/kiwi (~/.local/share/kiwi) elsewhere]
    #[arg(long, env = "KIWI_DATA_DIR")]
    pub data_dir: Option<PathBuf>,
    /// Built web app (web/dist) [default: <exe dir>/web]
    #[arg(long, env = "KIWI_WEB_DIR")]
    pub web_dir: Option<PathBuf>,
    /// HTTPS origin the phone uses; requests must name its host or the listen address [default: https://<this machine's Tailscale name>]
    #[arg(long, env = "KIWI_PUBLIC_URL")]
    pub public_url: Option<String>,
    /// tailscaled's LocalAPI socket; without it Kiwi runs the `tailscale` CLI (--tailscale-bin)
    #[arg(
        long,
        env = "KIWI_TAILSCALE_SOCKET",
        default_value = kiwi_device::TAILSCALED_SOCKET
    )]
    pub tailscale_socket: PathBuf,
    /// The `tailscale` CLI, for a Mac without tailscaled's socket (App Store or standalone app)
    #[arg(long, env = "KIWI_TAILSCALE_BIN", default_value = TAILSCALE_BIN)]
    pub tailscale_bin: PathBuf,
    /// Treat simulators as linkable devices (testing only).
    #[arg(long, env = "KIWI_ALLOW_SIMULATORS")]
    pub allow_simulators: bool,
    /// Tailscale login of the owner [default: the user who owns this machine in Tailscale]
    #[arg(long, env = "KIWI_OWNER")]
    pub owner: Option<String>,
    /// Python 3.12 or newer for Kiwi Builds' runners
    #[arg(long, env = "KIWI_PYTHON", default_value = PYTHON)]
    pub python: PathBuf,
}

/// Resolved settings that do not change at runtime.
#[derive(Debug, Clone)]
pub struct Config {
    /// Where Kiwi listens; requests may name it as their host (`127.0.0.1:<port>`).
    pub listen: SocketAddr,
    pub data_dir: PathBuf,
    pub web_dir: PathBuf,
    /// Without a trailing slash.
    pub public_url: Option<String>,
    pub allow_simulators: bool,
    /// `--owner`: the owner's Tailscale login, instead of asking Tailscale.
    pub owner: Option<String>,
    /// App Store country until one is chosen in Settings: the region of this machine's
    /// locale.
    pub country: Option<String>,
    /// `--python`: runs Kiwi Builds' runners.
    pub python: PathBuf,
}

/// `http(s)://host[:port][/path]`, returned without a trailing slash.
pub fn normalize_public_url(value: &str) -> Result<String, String> {
    let url = url::Url::parse(value.trim()).map_err(|e| format!("--public-url: {e}"))?;
    if !matches!(url.scheme(), "https" | "http")
        || url.host_str().is_none()
        || url.query().is_some()
    {
        return Err(format!(
            "--public-url must be an http(s) origin, got {value}"
        ));
    }
    Ok(url.as_str().trim_end_matches('/').to_owned())
}

/// A `Host` or `X-Forwarded-Host` value as (lowercase host without a trailing dot, port).
/// IPv6 hosts keep their brackets.
pub fn parse_host(value: &str) -> Option<(String, Option<u16>)> {
    let authority: axum::http::uri::Authority = value.trim().parse().ok()?;
    if authority.as_str().contains('@') {
        return None;
    }
    let host = authority.host().trim_end_matches('.').to_ascii_lowercase();
    (!host.is_empty()).then(|| (host, authority.port_u16()))
}

/// Host names a request may be addressed to: the public URL's host, or the listen address
/// on loopback (`127.0.0.1`, `localhost`, `[::1]` with Kiwi's port). Anything else (a DNS
/// rebinding page, for example) is rejected.
#[derive(Debug, Clone)]
pub struct AllowedHosts {
    /// Host, port, and the port a `Host` without one means (the scheme's default).
    public: (String, u16, u16),
    listen_port: u16,
    listen_ip: String,
}

impl AllowedHosts {
    pub fn new(public_url: &str, listen: SocketAddr) -> Self {
        let url = url::Url::parse(public_url).ok();
        let host = url
            .as_ref()
            .and_then(|u| u.host_str())
            .map(|h| h.trim_end_matches('.').to_ascii_lowercase())
            .unwrap_or_default();
        let port = url
            .as_ref()
            .and_then(url::Url::port_or_known_default)
            .unwrap_or(443);
        let implied = match url.as_ref().map(url::Url::scheme) {
            Some("http") => 80,
            _ => 443,
        };
        let listen_ip = match listen.ip() {
            std::net::IpAddr::V6(ip) => format!("[{ip}]"),
            ip => ip.to_string(),
        };
        Self {
            public: (host, port, implied),
            listen_port: listen.port(),
            listen_ip,
        }
    }

    fn is_public(&self, value: &str) -> bool {
        parse_host(value).is_some_and(|(host, port)| {
            host == self.public.0 && port.unwrap_or(self.public.2) == self.public.1
        })
    }

    fn is_listen(&self, value: &str) -> bool {
        parse_host(value).is_some_and(|(host, port)| {
            port.unwrap_or(80) == self.listen_port
                && (["127.0.0.1", "localhost", "[::1]"].contains(&host.as_str())
                    || host == self.listen_ip)
        })
    }

    /// `host`: the `Host` header (or URI authority); `forwarded`: `X-Forwarded-Host`, which
    /// must name the public host when present.
    pub fn allows(&self, host: Option<&str>, forwarded: Option<&str>) -> bool {
        host.is_some_and(|h| self.is_public(h) || self.is_listen(h))
            && forwarded.is_none_or(|f| self.is_public(f))
    }
}

/// A host (and port) from `X-Forwarded-Host`.
pub fn valid_forwarded_host(host: &str) -> bool {
    !host.is_empty()
        && host.len() <= 255
        && host
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b".-:[]".contains(&b))
}

impl Args {
    pub fn config(&self) -> Result<Config, String> {
        let data_dir = match &self.data_dir {
            Some(d) => d.clone(),
            None => default_data_dir(|key| std::env::var_os(key))
                .ok_or("HOME is not set; pass --data-dir")?,
        };
        let web_dir = match &self.web_dir {
            Some(d) => d.clone(),
            None => std::env::current_exe()
                .ok()
                .and_then(|exe| Some(exe.parent()?.join("web")))
                .ok_or("Cannot locate the executable; pass --web-dir")?,
        };
        Ok(Config {
            listen: self.listen,
            data_dir,
            web_dir,
            public_url: self
                .public_url
                .as_deref()
                .map(normalize_public_url)
                .transpose()?,
            allow_simulators: self.allow_simulators,
            owner: self
                .owner
                .as_deref()
                .map(str::trim)
                .filter(|o| !o.is_empty())
                .map(str::to_owned),
            country: if cfg!(target_os = "macos") {
                mac_region()
            } else {
                posix_region(|key| std::env::var(key).ok())
            },
            python: self.python.clone(),
        })
    }
}

/// `~/Library/Application Support/Kiwi` on macOS; elsewhere `$XDG_DATA_HOME/kiwi`, or
/// `~/.local/share/kiwi` when that is not set (the XDG Base Directory default). `env` reads
/// an environment variable.
fn default_data_dir(env: impl Fn(&str) -> Option<std::ffi::OsString>) -> Option<PathBuf> {
    let home = || env("HOME").filter(|h| !h.is_empty()).map(PathBuf::from);
    if cfg!(target_os = "macos") {
        return home().map(|h| h.join("Library/Application Support/Kiwi"));
    }
    // The specification ignores a relative XDG_DATA_HOME.
    env("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|d| d.is_absolute())
        .or_else(|| home().map(|h| h.join(".local/share")))
        .map(|d| d.join("kiwi"))
}

/// The region of the Mac's locale (`defaults read -g AppleLocale`), lowercase.
fn mac_region() -> Option<String> {
    let output = std::process::Command::new("/usr/bin/defaults")
        .args(["read", "-g", "AppleLocale"])
        .output()
        .ok()?;
    region_of_locale(&String::from_utf8_lossy(&output.stdout))
}

/// The region of the first of `LC_ALL`, `LC_MESSAGES` and `LANG` that names one, lowercase
/// (`nl_NL.UTF-8` -> `nl`). `C`, `POSIX` and `C.UTF-8` name none. `env` reads an
/// environment variable.
fn posix_region(env: impl Fn(&str) -> Option<String>) -> Option<String> {
    ["LC_ALL", "LC_MESSAGES", "LANG"].iter().find_map(|key| {
        // language[_territory][.codeset][@modifier]
        let value = env(key)?;
        region_of_locale(value.split(['.', '@']).next()?)
    })
}

/// `en_NL` -> `nl`; a region override wins (`en_US@rg=nlzzzz` -> `nl`).
pub fn region_of_locale(locale: &str) -> Option<String> {
    let locale = locale.trim();
    let (base, extensions) = locale.split_once('@').unwrap_or((locale, ""));
    let is_region = |r: &&str| r.len() == 2 && r.bytes().all(|b| b.is_ascii_alphabetic());
    extensions
        .split(';')
        .find_map(|e| e.strip_prefix("rg="))
        .and_then(|rg| rg.get(..2))
        .filter(is_region)
        .or_else(|| base.split(['_', '-']).skip(1).find(is_region))
        .map(str::to_ascii_lowercase)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn public_urls_and_hosts() {
        assert_eq!(
            normalize_public_url("https://mac-studio.example.ts.net/").unwrap(),
            "https://mac-studio.example.ts.net"
        );
        assert_eq!(
            normalize_public_url("http://127.0.0.1:8742").unwrap(),
            "http://127.0.0.1:8742"
        );
        assert!(normalize_public_url("ftp://x").is_err());
        assert!(normalize_public_url("nonsense").is_err());
        assert!(valid_forwarded_host("mac-studio.example.ts.net"));
        assert!(valid_forwarded_host("[::1]:8443"));
        assert!(!valid_forwarded_host("evil.com/path"));
        assert!(!valid_forwarded_host(""));
    }

    #[test]
    fn allowed_hosts() {
        let listen: SocketAddr = "127.0.0.1:8742".parse().unwrap();
        let hosts = AllowedHosts::new("https://mac-studio.example.ts.net", listen);
        let ts = "mac-studio.example.ts.net";
        // tailscale serve, and local curl.
        assert!(hosts.allows(Some(ts), Some(ts)));
        assert!(hosts.allows(Some("Mac-Studio.example.ts.net.:443"), None));
        assert!(hosts.allows(Some("127.0.0.1:8742"), None));
        assert!(hosts.allows(Some("127.0.0.1:8742"), Some(ts)));
        assert!(hosts.allows(Some("localhost:8742"), None));
        assert!(hosts.allows(Some("[::1]:8742"), None));
        // DNS rebinding and other names, ports or forwarded hosts.
        assert!(!hosts.allows(Some("evil.test:8742"), None));
        assert!(!hosts.allows(Some("evil.test"), Some(ts)));
        assert!(!hosts.allows(Some("127.0.0.1:8743"), None));
        assert!(!hosts.allows(Some("127.0.0.1"), None));
        assert!(!hosts.allows(Some(&format!("{ts}:8443")), None));
        assert!(!hosts.allows(Some("127.0.0.1:8742"), Some("evil.test")));
        assert!(!hosts.allows(Some(ts), Some("")));
        assert!(!hosts.allows(Some("user@127.0.0.1:8742"), None));
        assert!(!hosts.allows(None, None));
        let any: SocketAddr = "0.0.0.0:80".parse().unwrap();
        let hosts = AllowedHosts::new("http://kiwi.test:8080", any);
        assert!(hosts.allows(Some("kiwi.test:8080"), None));
        assert!(!hosts.allows(Some("kiwi.test"), None));
        assert!(hosts.allows(Some("localhost"), None));
    }

    #[test]
    fn parses_flags_with_defaults() {
        let args =
            Args::try_parse_from(["kiwi", "--data-dir", "/tmp/k", "--allow-simulators"]).unwrap();
        assert_eq!(args.listen.to_string(), "127.0.0.1:8742");
        assert!(args.allow_simulators);
        let config = args.config().unwrap();
        assert_eq!(config.data_dir, PathBuf::from("/tmp/k"));
        assert!(config.public_url.is_none());
    }

    #[test]
    fn platform_defaults() {
        let args = Args::try_parse_from(["kiwi"]).unwrap();
        if cfg!(target_os = "macos") {
            assert_eq!(
                args.tailscale_bin,
                PathBuf::from("/opt/homebrew/bin/tailscale")
            );
            assert_eq!(args.python, PathBuf::from("/opt/homebrew/bin/python3.13"));
        } else {
            assert_eq!(args.tailscale_bin, PathBuf::from("/usr/bin/tailscale"));
            assert_eq!(args.python, PathBuf::from("python3"));
        }
        let args = Args::try_parse_from(["kiwi", "--python", "/usr/bin/python3.13"]).unwrap();
        assert_eq!(args.python, PathBuf::from("/usr/bin/python3.13"));
    }

    #[test]
    fn default_data_directories() {
        let env = |vars: &'static [(&'static str, &'static str)]| {
            move |key: &str| {
                vars.iter()
                    .find(|(k, _)| *k == key)
                    .map(|(_, v)| std::ffi::OsString::from(v))
            }
        };
        let home = env(&[("HOME", "/home/alex")]);
        let xdg = env(&[("HOME", "/home/alex"), ("XDG_DATA_HOME", "/srv/data")]);
        let relative = env(&[("HOME", "/home/alex"), ("XDG_DATA_HOME", "data")]);
        if cfg!(target_os = "macos") {
            let mac = PathBuf::from("/home/alex/Library/Application Support/Kiwi");
            assert_eq!(default_data_dir(home), Some(mac.clone()));
            assert_eq!(default_data_dir(xdg), Some(mac));
        } else {
            let local = PathBuf::from("/home/alex/.local/share/kiwi");
            assert_eq!(default_data_dir(home), Some(local.clone()));
            assert_eq!(default_data_dir(xdg), Some(PathBuf::from("/srv/data/kiwi")));
            assert_eq!(default_data_dir(relative), Some(local));
            assert_eq!(
                default_data_dir(env(&[("XDG_DATA_HOME", "/srv/data")])),
                Some(PathBuf::from("/srv/data/kiwi"))
            );
        }
        assert_eq!(default_data_dir(env(&[])), None);
        assert_eq!(default_data_dir(env(&[("HOME", "")])), None);
    }

    #[test]
    fn regions_of_posix_locales() {
        let env = |vars: &'static [(&'static str, &'static str)]| {
            move |key: &str| {
                vars.iter()
                    .find(|(k, _)| *k == key)
                    .map(|(_, v)| (*v).to_owned())
            }
        };
        assert_eq!(
            posix_region(env(&[("LANG", "nl_NL.UTF-8")])).as_deref(),
            Some("nl")
        );
        assert_eq!(
            posix_region(env(&[("LANG", "de_AT@euro")])).as_deref(),
            Some("at")
        );
        assert_eq!(
            posix_region(env(&[("LANG", "sr_RS.UTF-8@latin")])).as_deref(),
            Some("rs")
        );
        assert_eq!(
            posix_region(env(&[("LC_ALL", "en_GB.UTF-8"), ("LANG", "nl_NL.UTF-8")])).as_deref(),
            Some("gb")
        );
        assert_eq!(
            posix_region(env(&[("LC_MESSAGES", "fr_BE"), ("LANG", "nl_NL")])).as_deref(),
            Some("be")
        );
        // Containers often set LANG=C.UTF-8: no region there, so the next one counts.
        assert_eq!(
            posix_region(env(&[("LC_ALL", "C.UTF-8"), ("LANG", "en_IE.UTF-8")])).as_deref(),
            Some("ie")
        );
        assert_eq!(posix_region(env(&[("LANG", "C.UTF-8")])), None);
        assert_eq!(posix_region(env(&[("LANG", "POSIX")])), None);
        assert_eq!(posix_region(env(&[("LANG", "")])), None);
        assert_eq!(posix_region(env(&[])), None);
    }

    #[test]
    fn regions_of_locales() {
        assert_eq!(region_of_locale("en_NL\n").as_deref(), Some("nl"));
        assert_eq!(region_of_locale("nl_BE").as_deref(), Some("be"));
        assert_eq!(region_of_locale("en-US").as_deref(), Some("us"));
        assert_eq!(region_of_locale("en_US@rg=dezzzz").as_deref(), Some("de"));
        assert_eq!(region_of_locale("zh-Hant_TW").as_deref(), Some("tw"));
        assert_eq!(region_of_locale("en"), None);
        assert_eq!(region_of_locale(""), None);
    }
}
