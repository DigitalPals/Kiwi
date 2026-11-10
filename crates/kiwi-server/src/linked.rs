//! Apps added by IPA link or download page instead of an AltStore source. Kiwi downloads the
//! IPA, builds a one-app catalog from its Info.plist and icon, and on every refresh checks
//! the page for a new IPA link and the IPA for a new file (`HEAD` validators).

use std::path::Path;

use chrono::{DateTime, Utc};
use kiwi_source::{
    CancellationToken, Catalog, DownloadPage, DownloadProgress, Error, IpaLink, Release, Result,
    Source, Url, Validators, source_key,
};
use sha2::{Digest, Sha256};

use crate::{Kiwi, ota::icon_path};

/// Icons taken from IPAs: `https://kiwi.invalid/icons/<sha256>.png`, stored in the icon
/// cache under that URL (so OTA manifests find them) and served at `/api/icons/<file>`.
const ICON_BASE: &str = "https://kiwi.invalid/icons/";

/// The URL the web app loads an icon from: `/api/icons/…` for icons taken from IPAs, else
/// Kiwi's copy of the source's artwork ([`crate::art`]).
pub fn client_icon(url: &str) -> String {
    match url.strip_prefix(ICON_BASE) {
        Some(file) => format!("/api/icons/{file}"),
        None => crate::art::client_url(url),
    }
}

/// The cached icon file for `/api/icons/<file>`; `None` for names Kiwi never makes.
pub fn icon_file(kiwi: &Kiwi, file: &str) -> Option<std::path::PathBuf> {
    let sha = file.strip_suffix(".png")?;
    (sha.len() == 64
        && sha
            .bytes()
            .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase()))
    .then(|| icon_path(&kiwi.paths.icons, &format!("{ICON_BASE}{file}")))
}

/// The source key of an app added by link: its page's, else its IPA's.
pub fn key(ipa: &Url, page: Option<&DownloadPage>) -> String {
    source_key(page.map_or(ipa, |p| &p.url))
}

/// Downloads the IPA and builds its source. `previous`: the source being refreshed, whose
/// icon is kept when the new IPA has none.
pub async fn build(
    kiwi: &Kiwi,
    ipa: Url,
    page: Option<&DownloadPage>,
    previous: Option<&Source>,
    cancel: &CancellationToken,
) -> Result<Source> {
    // Validators come from HEAD, like the later checks they are compared with (GET and
    // HEAD answers can differ in ETag form or length). A server without HEAD still works;
    // only updates of a file at the same URL go unnoticed then.
    let validators = match kiwi.sources.validators(&ipa, cancel).await {
        Ok(v) => v,
        Err(e) => {
            tracing::info!(ipa = %ipa, "no HEAD validators: {e}");
            Validators::default()
        }
    };
    let linked = kiwi
        .sources
        .download_linked(
            &ipa,
            &kiwi.paths.downloads,
            cancel,
            &DownloadProgress::default(),
        )
        .await?;
    let validators = if validators.is_empty() {
        linked.validators.clone()
    } else {
        validators
    };
    let size = linked.size;
    let artifact = linked.artifact;
    let sha256 = artifact.sha256.clone();
    let metadata = artifact.metadata.clone();
    // Kept where builds look for downloads, so the first install doesn't download it again.
    let path = kiwi.paths.downloads.join(format!("{sha256}.ipa"));
    artifact.persist(&path)?;
    let icon = match cache_icon(kiwi, &path).await {
        Some(url) => Some(url),
        None => page
            .and_then(|p| p.links.icon.clone())
            .or_else(|| previous_app(previous).and_then(|a| a.icon_url.clone())),
    };
    let name = metadata
        .name
        .clone()
        .or_else(|| previous_app(previous).map(|a| a.name.clone()))
        .unwrap_or_else(|| {
            metadata
                .bundle_id
                .rsplit('.')
                .next()
                .unwrap_or(&metadata.bundle_id)
                .to_owned()
        });
    let host = page
        .map_or(&ipa, |p| &p.url)
        .host_str()
        .unwrap_or_default()
        .trim_start_matches("www.")
        .to_owned();
    let date = validators
        .last_modified
        .as_deref()
        .and_then(|d| DateTime::parse_from_rfc2822(d).ok())
        .map_or_else(Utc::now, |d| d.with_timezone(&Utc));
    let release = Release {
        version: metadata.version.clone(),
        build: metadata.build.clone(),
        revision: None,
        date: Some(date.to_rfc3339()),
        notes: None,
        download_url: ipa.clone(),
        size: Some(size),
        sha256: Some(sha256),
        min_os: metadata.min_os.clone(),
        max_os: None,
        platforms: metadata.platforms.clone(),
    };
    let app = kiwi_source::App {
        name: name.clone(),
        bundle_id: metadata.bundle_id.clone(),
        developer: None,
        subtitle: None,
        description: None,
        icon_url: icon.clone(),
        screenshots: vec![],
        tint_color: None,
        category: None,
        permissions: Default::default(),
        releases: vec![release],
        unavailable: None,
    };
    let url = page.map_or_else(|| ipa.clone(), |p| p.url.clone());
    Ok(Source {
        key: key(&ipa, page),
        url: url.clone(),
        enabled: true,
        // Named after the site, like a source's publisher; the app's name is its subtitle.
        catalog: Catalog {
            name: host,
            identifier: None,
            source_url: Some(url.to_string()),
            subtitle: Some(name),
            description: None,
            icon_url: icon,
            apps: vec![app],
            diagnostics: page
                .filter(|p| p.links.ipas.len() > 1)
                .map(|p| {
                    vec![format!(
                        "The page links {} IPAs; Kiwi uses the first one.",
                        p.links.ipas.len()
                    )]
                })
                .unwrap_or_default(),
        },
        etag: page.and_then(|p| p.etag.clone()),
        last_modified: page.and_then(|p| p.last_modified.clone()),
        last_success: Some(Utc::now()),
        error: None,
        link: Some(IpaLink {
            ipa,
            page: page.map(|p| p.url.clone()),
            validators,
        }),
    })
}

fn previous_app(previous: Option<&Source>) -> Option<&kiwi_source::App> {
    previous.and_then(|s| s.catalog.apps.first())
}

/// Checks the page and the IPA, and downloads the IPA again only when it changed. On error
/// the caller keeps the previous catalog and records the error.
pub async fn refresh(kiwi: &Kiwi, source: &Source, cancel: &CancellationToken) -> Result<Source> {
    let link = source
        .link
        .as_ref()
        .ok_or_else(|| Error::new("Source fetch", "Not an IPA link"))?;
    let mut ipa = link.ipa.clone();
    let mut page = None;
    if let Some(url) = &link.page {
        let previous = (source.etag.clone(), source.last_modified.clone());
        if let Some(fresh) = kiwi.sources.page(url, previous, cancel).await? {
            ipa = fresh.links.ipas.first().cloned().ok_or_else(|| {
                Error::new(
                    "Source fetch",
                    "The download page no longer links an IPA. Check the page, or remove this app's source.",
                )
            })?;
            page = Some(fresh);
        }
    }
    if ipa == link.ipa {
        let now = match kiwi.sources.validators(&ipa, cancel).await {
            // Servers that don't answer HEAD: only a new link on the page is noticed.
            Err(e) if e.detail.contains("HTTP 405") || e.detail.contains("HTTP 501") => {
                Validators::default()
            }
            result => result?,
        };
        let same =
            now.is_empty() || link.validators.is_empty() || !now.changed_from(&link.validators);
        if same {
            let mut next = source.clone();
            if let Some(page) = &page {
                next.etag = page.etag.clone();
                next.last_modified = page.last_modified.clone();
            }
            next.last_success = Some(Utc::now());
            next.error = None;
            return Ok(next);
        }
    }
    let page = match page {
        Some(page) => Some(page),
        // The IPA changed behind an unchanged page: keep the page's validators.
        None => link.page.clone().map(|url| DownloadPage {
            url,
            links: Default::default(),
            etag: source.etag.clone(),
            last_modified: source.last_modified.clone(),
        }),
    };
    tracing::info!(ipa = %ipa, "IPA link changed; downloading it again");
    let mut next = build(kiwi, ipa, page.as_ref(), Some(source), cancel).await?;
    next.enabled = source.enabled;
    Ok(next)
}

/// Stores the IPA's icon in the icon cache; its URL, or `None` when it has none Kiwi can
/// read.
async fn cache_icon(kiwi: &Kiwi, ipa: &Path) -> Option<Url> {
    let path = ipa.to_owned();
    let png = match tokio::task::spawn_blocking(move || kiwi_source::ipa_icon(&path)).await {
        Ok(Ok(Some(png))) => png,
        Ok(Ok(None)) => return None,
        Ok(Err(e)) => {
            tracing::info!("could not read the IPA's icon: {e}");
            return None;
        }
        Err(e) => {
            tracing::warn!("icon extraction failed: {e}");
            return None;
        }
    };
    let url = Url::parse(&format!(
        "{ICON_BASE}{}.png",
        hex::encode(Sha256::digest(&png))
    ))
    .ok()?;
    let file = icon_path(&kiwi.paths.icons, url.as_str());
    if !file.is_file() {
        let tmp = file.with_extension("tmp");
        let written = async {
            tokio::fs::create_dir_all(&kiwi.paths.icons).await?;
            tokio::fs::write(&tmp, &png).await?;
            tokio::fs::rename(&tmp, &file).await
        };
        if let Err(e) = written.await {
            tracing::warn!("could not cache the IPA's icon: {e}");
            return None;
        }
    }
    Some(url)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn icon_urls_for_the_web_app() {
        let sha = "a".repeat(64);
        assert_eq!(
            client_icon(&format!("{ICON_BASE}{sha}.png")),
            format!("/api/icons/{sha}.png")
        );
        assert_eq!(
            client_icon("https://example.com/i.png"),
            crate::art::client_url("https://example.com/i.png")
        );
    }
}
