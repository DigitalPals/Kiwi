use crate::{
    CancellationToken, Error, PageLinks, Result, Source, SourceInput, Url, github_raw,
    is_public_ip, network_url, page_links, parse_catalog, public_url, source_input, source_key,
    urls::BLOCKED,
};
use reqwest::{
    Client, StatusCode,
    dns::{Addrs, Name, Resolve, Resolving},
    header::{ETAG, IF_MODIFIED_SINCE, IF_NONE_MATCH, LAST_MODIFIED},
};
use std::{error::Error as _, net::SocketAddr, sync::Arc, time::Duration};
use tokio::sync::Semaphore;

pub(crate) fn request_error(stage: &'static str, error: reqwest::Error) -> Error {
    let blocked =
        std::iter::successors(error.source(), |e| (*e).source()).any(|e| e.is::<BlockedAddress>());
    let detail = if blocked {
        BLOCKED
    } else if error.is_timeout() {
        "Request timed out. Check the connection and retry."
    } else if error.is_connect() {
        "Could not connect. Check the connection, proxy, and source address."
    } else if error.is_redirect() {
        "Redirect failed or exceeded the redirect limit."
    } else if error.is_body() || error.is_decode() {
        "Incomplete or invalid HTTP response body. Retry the download."
    } else {
        "HTTP request failed. Check the source address and retry."
    };
    Error::new(stage, detail)
}

pub const MAX_FEED_BYTES: u64 = 8 * 1024 * 1024;
pub const MAX_IMAGE_BYTES: u64 = 2 * 1024 * 1024;
pub const MAX_IPA_BYTES: u64 = 4 * 1024 * 1024 * 1024;

#[derive(Debug, thiserror::Error)]
#[error("{BLOCKED}")]
struct BlockedAddress;

/// Resolves host names and refuses the connection when any address is not public, so a
/// public name cannot point requests at the Mac, the LAN or the tailnet.
struct PublicResolver;
impl Resolve for PublicResolver {
    fn resolve(&self, name: Name) -> Resolving {
        let host = name.as_str().to_owned();
        Box::pin(async move {
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host, 0)).await?.collect();
            if addrs.is_empty() || addrs.iter().any(|a| !is_public_ip(a.ip())) {
                return Err(BlockedAddress.into());
            }
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

#[derive(Clone, Copy, Debug)]
pub struct ClientOptions {
    /// Maximum time between body chunks.
    pub read_timeout: Duration,
    /// Allow plain HTTP and loopback/private targets (tests and local catalogs only).
    /// Otherwise every request and redirect must pass [`public_url`], resolved addresses
    /// must be public, and environment proxies are ignored.
    pub allow_local: bool,
}
impl Default for ClientOptions {
    fn default() -> Self {
        Self {
            read_timeout: Duration::from_secs(30),
            allow_local: false,
        }
    }
}

#[derive(Clone, Debug)]
pub struct SourceClient {
    pub(crate) client: Client,
    pub(crate) permits: Arc<Semaphore>,
    allow_local: bool,
}
impl SourceClient {
    pub fn new() -> Result<Self> {
        Self::with_options(ClientOptions::default())
    }
    pub fn with_options(options: ClientOptions) -> Result<Self> {
        let allow_local = options.allow_local;
        let mut builder = Client::builder()
            .user_agent(concat!("Kiwi/", env!("CARGO_PKG_VERSION")))
            .connect_timeout(Duration::from_secs(15))
            .read_timeout(options.read_timeout)
            .redirect(reqwest::redirect::Policy::custom(move |attempt| {
                if attempt.previous().len() >= 8 {
                    attempt.error("Too many redirects")
                } else if network_url(attempt.url().as_str()).is_err()
                    || (!allow_local && public_url(attempt.url()).is_err())
                {
                    attempt.error("Unsafe redirect URL")
                } else if attempt
                    .previous()
                    .last()
                    .is_some_and(|u| u.scheme() == "https")
                    && attempt.url().scheme() != "https"
                {
                    attempt.error("HTTPS downgrade redirect rejected")
                } else {
                    attempt.follow()
                }
            }));
        if !allow_local {
            builder = builder
                .https_only(true)
                .no_proxy()
                .dns_resolver(PublicResolver);
        }
        let client = builder
            .build()
            .map_err(|e| Error::new("Network", e.without_url()))?;
        Ok(Self {
            client,
            permits: Arc::new(Semaphore::new(4)),
            allow_local,
        })
    }
    /// Checks a URL against this client's network policy before any request.
    pub fn check_url(&self, url: &Url) -> Result<()> {
        network_url(url.as_str())?;
        if !self.allow_local {
            public_url(url)?;
        }
        Ok(())
    }
    /// Resolves user input (GitHub blob, raw or any HTTPS URL) to the URL to store and fetch.
    pub async fn canonical_url(&self, input: &str, cancel: &CancellationToken) -> Result<Url> {
        let url = self.resolve_input(input, cancel).await?;
        self.check_url(&url)?;
        Ok(url)
    }
    async fn resolve_input(&self, input: &str, cancel: &CancellationToken) -> Result<Url> {
        match source_input(input)? {
            SourceInput::Canonical(u) => Ok(u),
            SourceInput::Github { owner, repo, tail } => {
                let mut matches = vec![];
                for split in 1..tail.len() {
                    let reference = tail[..split].join("/");
                    let mut api = Url::parse("https://api.github.com/").unwrap();
                    api.path_segments_mut()
                        .unwrap()
                        .clear()
                        .extend(["repos", &owner, &repo, "contents"])
                        .extend(&tail[split..]);
                    api.query_pairs_mut().append_pair("ref", &reference);
                    let response = tokio::select! { _=cancel.cancelled()=>return Err(Error::new("Source fetch","Cancelled")),
                    r=self.client.get(api).timeout(Duration::from_secs(45)).send()=>r.map_err(|e|crate::network::request_error("Source fetch",e))? };
                    if response.status() == StatusCode::NOT_FOUND {
                        continue;
                    }
                    if !response.status().is_success() {
                        return Err(Error::new(
                            "Source fetch",
                            format!(
                                "GitHub ref resolution returned HTTP {}. Use the raw URL, or retry later.",
                                response.status()
                            ),
                        ));
                    }
                    let bytes =
                        limited_body(response, MAX_FEED_BYTES, cancel, "Source fetch").await?;
                    let v: serde_json::Value = serde_json::from_slice(&bytes)
                        .map_err(|e| Error::new("Source fetch", e))?;
                    if v["type"] == "file" {
                        matches.push(github_raw(&owner, &repo, &reference, &tail[split..])?);
                    }
                }
                if matches.len() != 1 {
                    return Err(Error::new(
                        "Source URL",
                        "GitHub branch/path is ambiguous or unavailable. Copy the file's Raw URL.",
                    ));
                }
                Ok(matches.remove(0))
            }
        }
    }
    /// Resolves and fetches a new source. The caller checks for duplicates by `key`.
    pub async fn add(&self, input: &str, cancel: &CancellationToken) -> Result<Source> {
        let url = self.canonical_url(input, cancel).await?;
        self.fetch(url, None, cancel)
            .await?
            .ok_or_else(|| Error::new("Source fetch", "Unexpected 304 for a new source"))
    }
    /// Conditional refetch (ETag/Last-Modified). On error the caller keeps the previous
    /// catalog and records the error.
    pub async fn refresh(&self, source: &Source, cancel: &CancellationToken) -> Result<Source> {
        match self.fetch(source.url.clone(), Some(source), cancel).await? {
            Some(mut fresh) => {
                fresh.enabled = source.enabled;
                Ok(fresh)
            }
            None => {
                let mut s = source.clone();
                s.last_success = Some(chrono::Utc::now());
                s.error = None;
                Ok(s)
            }
        }
    }
    async fn fetch(
        &self,
        url: Url,
        previous: Option<&Source>,
        cancel: &CancellationToken,
    ) -> Result<Option<Source>> {
        let validators = previous.map(|old| (old.etag.clone(), old.last_modified.clone()));
        let Some(response) = self.get(&url, validators, cancel).await? else {
            return Ok(None);
        };
        if is_html(&response) {
            return Err(Error::new(
                "Source fetch",
                "Server returned HTML, not JSON. Check the raw URL or whether the source requires login.",
            ));
        }
        let body = limited_body(response.response, MAX_FEED_BYTES, cancel, "Source fetch").await?;
        self.parse_source(url, body, response.etag, response.last_modified, cancel)
            .await
            .map(Some)
    }
    /// A `GET` for a feed or page; `None` for `304 Not Modified` when `validators` (ETag,
    /// Last-Modified) were sent.
    async fn get(
        &self,
        url: &Url,
        validators: Option<(Option<String>, Option<String>)>,
        cancel: &CancellationToken,
    ) -> Result<Option<Fetched>> {
        self.check_url(url)?;
        let _permit = tokio::select! { _=cancel.cancelled()=>return Err(Error::new("Source fetch","Cancelled")),p=self.permits.acquire()=>p.map_err(|e|Error::new("Source fetch",e))? };
        let mut request = self
            .client
            .get(url.clone())
            .timeout(Duration::from_secs(60));
        let conditional = validators.is_some();
        if let Some((etag, last)) = validators {
            if let Some(etag) = etag {
                request = request.header(IF_NONE_MATCH, etag);
            }
            if let Some(last) = last {
                request = request.header(IF_MODIFIED_SINCE, last);
            }
        }
        let response = tokio::select! { _=cancel.cancelled()=>return Err(Error::new("Source fetch","Cancelled")),r=request.send()=>r.map_err(|e|crate::network::request_error("Source fetch",e))? };
        if response.status() == StatusCode::NOT_MODIFIED && conditional {
            return Ok(None);
        }
        if !response.status().is_success() {
            return Err(Error::new(
                "Source fetch",
                format!(
                    "HTTP {}. Check the source address or retry later.",
                    response.status()
                ),
            ));
        }
        let header = |name| {
            response
                .headers()
                .get(name)
                .and_then(|s| s.to_str().ok())
                .map(str::to_owned)
        };
        Ok(Some(Fetched {
            etag: header(ETAG),
            last_modified: header(LAST_MODIFIED),
            response,
        }))
    }
    async fn parse_source(
        &self,
        url: Url,
        bytes: Vec<u8>,
        etag: Option<String>,
        last_modified: Option<String>,
        cancel: &CancellationToken,
    ) -> Result<Source> {
        let base = url.clone();
        let catalog = tokio::task::spawn_blocking(move || parse_catalog(&bytes, &base))
            .await
            .map_err(|e| Error::new("Source parse", e))??;
        if cancel.is_cancelled() {
            return Err(Error::new("Source fetch", "Cancelled"));
        }
        Ok(Source {
            key: source_key(&url),
            url,
            enabled: true,
            catalog,
            etag,
            last_modified,
            last_success: Some(chrono::Utc::now()),
            error: None,
            link: None,
        })
    }
    /// What the URL a user pasted offers: an AltStore source, a download page (its
    /// AltStore source when it links one, otherwise its first IPA link) or an IPA.
    pub async fn find(&self, url: Url, cancel: &CancellationToken) -> Result<Found> {
        if url.path().to_ascii_lowercase().ends_with(".ipa") {
            self.check_url(&url)?;
            return Ok(Found::Ipa {
                ipa: url,
                page: None,
            });
        }
        let fetched = self
            .get(&url, None, cancel)
            .await?
            .ok_or_else(|| Error::new("Source fetch", "Unexpected 304 for a new source"))?;
        if is_html(&fetched) {
            let page = self.read_page(url, fetched, cancel).await?;
            if let Some(source) = page.links.sources.first() {
                let source = self
                    .fetch(source.clone(), None, cancel)
                    .await
                    .map_err(|e| {
                        Error::new(
                            e.stage,
                            format!(
                                "The page links an AltStore source ({source}), but: {}",
                                e.detail
                            ),
                        )
                    })?;
                return source
                    .map(Found::Source)
                    .ok_or_else(|| Error::new("Source fetch", "Unexpected 304 for a new source"));
            }
            let Some(ipa) = page.links.ipas.first().cloned() else {
                return Err(Error::new(
                    "Source fetch",
                    "This page links no AltStore source and no IPA. Paste the source's or the IPA's own URL.",
                ));
            };
            return Ok(Found::Ipa {
                ipa,
                page: Some(page),
            });
        }
        let mut response = fetched.response;
        // An IPA (a ZIP archive) behind a URL without the extension.
        let first = tokio::select! { _=cancel.cancelled()=>return Err(Error::new("Source fetch","Cancelled")),r=response.chunk()=>r.map_err(|e|request_error("Source fetch",e))? };
        let first = first.map(|b| b.to_vec()).unwrap_or_default();
        if first.starts_with(b"PK\x03\x04") {
            return Ok(Found::Ipa {
                ipa: url,
                page: None,
            });
        }
        let body =
            limited_body_after(response, first, MAX_FEED_BYTES, cancel, "Source fetch").await?;
        self.parse_source(url, body, fetched.etag, fetched.last_modified, cancel)
            .await
            .map(Found::Source)
    }
    /// A download page's links now; `None` when it has not changed since `previous`
    /// (its ETag and Last-Modified).
    pub async fn page(
        &self,
        url: &Url,
        previous: (Option<String>, Option<String>),
        cancel: &CancellationToken,
    ) -> Result<Option<DownloadPage>> {
        let Some(fetched) = self.get(url, Some(previous), cancel).await? else {
            return Ok(None);
        };
        if !is_html(&fetched) {
            return Err(Error::new(
                "Source fetch",
                "The download page is no longer a web page.",
            ));
        }
        self.read_page(url.clone(), fetched, cancel).await.map(Some)
    }
    async fn read_page(
        &self,
        url: Url,
        fetched: Fetched,
        cancel: &CancellationToken,
    ) -> Result<DownloadPage> {
        let base = fetched.response.url().clone();
        let body = limited_body(
            fetched.response,
            crate::page::MAX_PAGE_BYTES as u64,
            cancel,
            "Source fetch",
        )
        .await?;
        let html = String::from_utf8_lossy(&body);
        Ok(DownloadPage {
            links: page_links(&html, &base),
            url,
            etag: fetched.etag,
            last_modified: fetched.last_modified,
        })
    }
    /// Fetches artwork (icons, screenshots), at most [`MAX_IMAGE_BYTES`].
    pub async fn image(&self, url: &Url, cancel: &CancellationToken) -> Result<Vec<u8>> {
        self.check_url(url)?;
        let _permit = tokio::select! { _=cancel.cancelled()=>return Err(Error::new("Image","Cancelled")),p=self.permits.acquire()=>p.map_err(|e|Error::new("Image",e))? };
        let response = tokio::select! { _=cancel.cancelled()=>return Err(Error::new("Image","Cancelled")),r=self.client.get(url.clone()).timeout(Duration::from_secs(30)).send()=>r.map_err(|e|crate::network::request_error("Image",e))? };
        if !response.status().is_success() || is_html(&response) {
            return Err(Error::new("Image", "Artwork unavailable"));
        }
        limited_body(response, MAX_IMAGE_BYTES, cancel, "Image").await
    }
    /// Fetches a small JSON document (e.g. an App Store lookup) under the same network
    /// policy, at most `max` bytes. `stage` names the request in errors.
    pub async fn json_bytes(
        &self,
        stage: &'static str,
        url: &Url,
        max: u64,
        cancel: &CancellationToken,
    ) -> Result<Vec<u8>> {
        self.check_url(url)?;
        let cancelled = || Error::new(stage, "Cancelled");
        let _permit = tokio::select! {
            _ = cancel.cancelled() => return Err(cancelled()),
            p = self.permits.acquire() => p.map_err(|e| Error::new(stage, e))?,
        };
        let request = self
            .client
            .get(url.clone())
            .header(reqwest::header::ACCEPT, "application/json")
            .timeout(Duration::from_secs(30));
        let response = tokio::select! {
            _ = cancel.cancelled() => return Err(cancelled()),
            r = request.send() => r.map_err(|e| request_error(stage, e))?,
        };
        if !response.status().is_success() {
            return Err(Error::new(stage, format!("HTTP {}", response.status())));
        }
        if is_html(&response) {
            return Err(Error::new(stage, "Expected JSON, got a web page"));
        }
        limited_body(response, max, cancel, stage).await
    }
}
/// A successful `GET`, with the validators for the next conditional one.
pub(crate) struct Fetched {
    response: reqwest::Response,
    etag: Option<String>,
    last_modified: Option<String>,
}
impl std::ops::Deref for Fetched {
    type Target = reqwest::Response;
    fn deref(&self) -> &reqwest::Response {
        &self.response
    }
}

/// What a pasted URL offers (see [`SourceClient::find`]).
#[derive(Debug)]
#[allow(clippy::large_enum_variant)] // One per added source.
pub enum Found {
    /// An AltStore source, possibly the one a download page links.
    Source(Source),
    /// An IPA, and the download page it was found on.
    Ipa {
        ipa: Url,
        page: Option<DownloadPage>,
    },
}

/// A download page and what it links.
#[derive(Debug, Clone)]
pub struct DownloadPage {
    /// The page's URL as added (before redirects).
    pub url: Url,
    pub links: PageLinks,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
}

pub(crate) fn is_html(response: &reqwest::Response) -> bool {
    response
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|h| h.to_str().ok())
        .is_some_and(|s| s.contains("text/html") || s.contains("application/xhtml"))
}
pub(crate) async fn limited_body(
    response: reqwest::Response,
    max: u64,
    cancel: &CancellationToken,
    stage: &'static str,
) -> Result<Vec<u8>> {
    if response.content_length().is_some_and(|n| n > max) {
        return Err(Error::new(stage, "Response exceeds size limit"));
    }
    limited_body_after(response, vec![], max, cancel, stage).await
}
/// The rest of a body whose start (`bytes`) was already read.
async fn limited_body_after(
    mut response: reqwest::Response,
    mut bytes: Vec<u8>,
    max: u64,
    cancel: &CancellationToken,
    stage: &'static str,
) -> Result<Vec<u8>> {
    if bytes.len() as u64 > max {
        return Err(Error::new(stage, "Response exceeds size limit"));
    }
    loop {
        let chunk = tokio::select! { _=cancel.cancelled()=>return Err(Error::new(stage,"Cancelled")),r=response.chunk()=>r.map_err(|e|request_error(stage,e))? };
        let Some(chunk) = chunk else {
            break;
        };
        if bytes.len() as u64 + chunk.len() as u64 > max {
            return Err(Error::new(stage, "Response exceeds size limit"));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}
