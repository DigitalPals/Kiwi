use crate::{Error, Result, Url};
use percent_encoding::percent_decode_str;
use std::net::{IpAddr, Ipv4Addr};
use url::Host;

/// Validate network URLs at the boundary, including redirects. Userinfo is never accepted.
pub fn network_url(value: &str) -> Result<Url> {
    let url = Url::parse(value).map_err(|e| Error::new("URL", e))?;
    if !matches!(url.scheme(), "https" | "http")
        || url.host_str().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return Err(Error::new(
            "URL",
            "Expected an HTTP(S) URL without embedded credentials",
        ));
    }
    Ok(url)
}

/// The default [`crate::SourceClient`] policy: HTTPS only, and IP literals must be public.
/// Host names are checked again after DNS resolution.
pub fn public_url(url: &Url) -> Result<()> {
    if url.scheme() != "https" {
        return Err(Error::new("URL", "Only HTTPS URLs are allowed"));
    }
    let ip: IpAddr = match url.host() {
        Some(Host::Domain(_)) => return Ok(()),
        Some(Host::Ipv4(ip)) => ip.into(),
        Some(Host::Ipv6(ip)) => ip.into(),
        None => return Err(Error::new("URL", "Missing host")),
    };
    if !is_public_ip(ip) {
        return Err(Error::new("URL", BLOCKED));
    }
    Ok(())
}

pub(crate) const BLOCKED: &str = "Refusing to connect to a private or local network address";

/// Globally routable unicast only: no loopback, private, link-local, CGNAT (Tailscale),
/// multicast, documentation or reserved ranges, including IPv4 embedded in IPv6.
pub fn is_public_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let [a, b, c, _] = ip.octets();
            !(a == 0
                || a >= 240
                || ip.is_loopback()
                || ip.is_private()
                || ip.is_link_local()
                || ip.is_multicast()
                || ip.is_documentation()
                || (a == 100 && (64..128).contains(&b))
                || (a == 192 && b == 0 && c == 0)
                || (a == 198 && (18..20).contains(&b)))
        }
        IpAddr::V6(ip) => {
            if let Some(v4) = ip.to_ipv4_mapped() {
                return is_public_ip(v4.into());
            }
            let s = ip.segments();
            let embedded = |hi: u16, lo: u16| {
                is_public_ip(Ipv4Addr::from((u32::from(hi) << 16) | u32::from(lo)).into())
            };
            match s {
                // 6to4 and NAT64 (well-known prefix) carry an IPv4 destination.
                [0x2002, hi, lo, ..] => embedded(hi, lo),
                [0x64, 0xff9b, 0, 0, 0, 0, hi, lo] => embedded(hi, lo),
                // Global unicast, minus IETF assignments (incl. Teredo) and documentation.
                [first, second, ..] => {
                    first & 0xe000 == 0x2000
                        && !(first == 0x2001 && (second < 0x200 || second == 0xdb8))
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SourceInput {
    Canonical(Url),
    /// More than one ref/path split is possible. Resolve through GitHub's contents API.
    Github {
        owner: String,
        repo: String,
        tail: Vec<String>,
    },
}

/// Whether the URL names this machine (`localhost` or a loopback address literal).
pub(crate) fn is_loopback(url: &Url) -> bool {
    matches!(url.host_str(), Some("localhost" | "127.0.0.1" | "[::1]"))
}

/// What a user pasted, as the URL to fetch. A missing scheme means HTTPS, and plain HTTP is
/// upgraded to HTTPS (Kiwi never fetches over HTTP except from loopback, for local
/// catalogs and tests).
pub fn source_input(value: &str) -> Result<SourceInput> {
    let value = value.trim();
    let with_scheme;
    let value = if value.contains("://") {
        value
    } else {
        with_scheme = format!("https://{value}");
        &with_scheme
    };
    let mut url = network_url(value)?;
    if url.scheme() != "https" && !is_loopback(&url) {
        url.set_scheme("https")
            .map_err(|()| Error::new("Source URL", "Use an HTTPS URL"))?;
    }
    url.set_fragment(None);
    if matches!(url.host_str(), Some("github.com" | "www.github.com")) {
        let parts: Vec<String> = url
            .path_segments()
            .into_iter()
            .flatten()
            .map(|s| percent_decode_str(s).decode_utf8().map(|s| s.into_owned()))
            .collect::<std::result::Result<_, _>>()
            .map_err(|e| Error::new("Source URL", e))?;
        if parts.len() < 5 || parts[2] != "blob" || parts.iter().any(|s| s.is_empty()) {
            return Err(Error::new(
                "Source URL",
                "Use a GitHub file's blob URL or its raw HTTPS URL",
            ));
        }
        let owner = parts[0].clone();
        let repo = parts[1].clone();
        let tail = parts[3..].to_vec();
        // A percent-encoded slash in a ref is already an unambiguous single segment.
        if tail.len() == 2 {
            return Ok(SourceInput::Canonical(github_raw(
                &owner,
                &repo,
                &tail[0],
                &tail[1..],
            )?));
        }
        if tail.len() > 16 {
            return Err(Error::new(
                "Source URL",
                "Ambiguous GitHub path is too long; use the raw URL",
            ));
        }
        return Ok(SourceInput::Github { owner, repo, tail });
    }
    if url.host_str() == Some("raw.githubusercontent.com") {
        // Only GitHub source URLs have known presentation/tracking query parameters.
        let pairs: Vec<_> = url
            .query_pairs()
            .filter(|(k, _)| {
                !k.starts_with("utm_") && !matches!(k.as_ref(), "raw" | "fbclid" | "gclid")
            })
            .map(|(k, v)| (k.into_owned(), v.into_owned()))
            .collect();
        url.set_query(None);
        if !pairs.is_empty() {
            url.query_pairs_mut().extend_pairs(pairs);
        }
    }
    Ok(SourceInput::Canonical(url))
}

pub fn github_raw(owner: &str, repo: &str, reference: &str, path: &[String]) -> Result<Url> {
    let mut url = Url::parse("https://raw.githubusercontent.com/").unwrap();
    url.path_segments_mut()
        .unwrap()
        .clear()
        .extend([owner, repo, reference])
        .extend(path);
    Ok(url)
}

pub fn relative_url(base: &Url, value: &str) -> Result<Url> {
    network_url(base.join(value).map_err(|e| Error::new("URL", e))?.as_str())
}
