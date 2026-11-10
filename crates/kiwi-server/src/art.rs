//! Source artwork (icons, screenshots) served by Kiwi instead of loaded by the phone from
//! the hosts a source names: those hosts would otherwise see each phone's address, and the
//! fetch goes through kiwi-source's network policy (HTTPS, public addresses, size limit)
//! like every other request Kiwi makes.
//!
//! The web app gets `/api/art/<mac>?url=<artwork URL>`, where `mac` is an HMAC of the URL
//! with Kiwi's own key (`<data>/art.key`, 0600, never sent anywhere): other web pages can't
//! make the owner's phone ask Kiwi to fetch URLs of their choosing. Fetched images are kept
//! in `<data>/art` and deleted after [`RETENTION`] (they are fetched again when shown);
//! artwork that can't be fetched isn't asked for again for [`RETRY_AFTER`].

use std::{
    collections::HashMap,
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
    sync::{LazyLock, Mutex, OnceLock},
    time::{Duration, Instant, SystemTime},
};

use hmac::{Hmac, KeyInit, Mac};
use kiwi_source::{CancellationToken, Url};
use sha2::{Digest, Sha256};

use crate::Kiwi;

/// Cached artwork older than this is deleted by the hourly cleanup.
pub const RETENTION: Duration = Duration::from_secs(30 * 24 * 3600);
/// How long Kiwi answers 404 for artwork it couldn't fetch, without trying again.
pub const RETRY_AFTER: Duration = Duration::from_secs(15 * 60);

static KEY: OnceLock<[u8; 32]> = OnceLock::new();

fn random_bytes<const N: usize>() -> [u8; N] {
    let mut bytes = [0u8; N];
    getrandom::fill(&mut bytes).expect("system random number generator");
    bytes
}

/// Loads the key from `path`, making it first if there's none (or it's unreadable), so
/// artwork URLs stay the same across restarts. Without it (an error, or in tests) the key
/// is random for this process.
pub fn load_key(path: &Path) -> Result<(), String> {
    let error = |e: std::io::Error| format!("{}: {e}", path.display());
    let stored = std::fs::read_to_string(path)
        .ok()
        .and_then(|text| hex::decode(text.trim()).ok())
        .and_then(|bytes| <[u8; 32]>::try_from(bytes).ok());
    let key = match stored {
        Some(key) => key,
        None => {
            let key = random_bytes::<32>();
            let tmp = path.with_extension("tmp");
            let _ = std::fs::remove_file(&tmp);
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&tmp)
                .map_err(error)?;
            file.write_all(hex::encode(key).as_bytes())
                .and_then(|()| file.sync_all())
                .map_err(error)?;
            std::fs::rename(&tmp, path).map_err(error)?;
            key
        }
    };
    let _ = KEY.set(key);
    Ok(())
}

fn mac(url: &str) -> Hmac<Sha256> {
    let key = KEY.get_or_init(random_bytes::<32>);
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("any key length");
    mac.update(url.as_bytes());
    mac
}

/// The URL the web app loads artwork at `url` from.
pub fn client_url(url: &str) -> String {
    let tag = hex::encode(mac(url).finalize().into_bytes());
    let query: String = url::form_urlencoded::byte_serialize(url.as_bytes()).collect();
    format!("/api/art/{tag}?url={query}")
}

/// Whether `tag` is the one [`client_url`] made for `url`.
pub fn verify(tag: &str, url: &str) -> bool {
    hex::decode(tag).is_ok_and(|tag| mac(url).verify_slice(&tag).is_ok())
}

/// The content type of image bytes Kiwi serves as artwork, by their signature.
pub fn image_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else {
        None
    }
}

fn cache_path(dir: &Path, url: &str) -> PathBuf {
    dir.join(hex::encode(Sha256::digest(url.as_bytes())))
}

/// Artwork URLs that couldn't be fetched, and when.
static FAILED: LazyLock<Mutex<HashMap<String, Instant>>> = LazyLock::new(Mutex::default);

fn failed_recently(url: &str) -> bool {
    let failed = FAILED.lock().unwrap_or_else(|e| e.into_inner());
    failed.get(url).is_some_and(|at| at.elapsed() < RETRY_AFTER)
}

fn remember_failure(url: &str) {
    let mut failed = FAILED.lock().unwrap_or_else(|e| e.into_inner());
    failed.retain(|_, at| at.elapsed() < RETRY_AFTER);
    failed.insert(url.to_owned(), Instant::now());
}

/// The artwork at `url` and its content type: from the cache, else fetched and cached.
/// `None` when it can't be fetched or isn't an image (remembered for [`RETRY_AFTER`]).
pub async fn get(kiwi: &Kiwi, url: &str) -> Option<(Vec<u8>, &'static str)> {
    let path = cache_path(&kiwi.paths.art, url);
    if let Ok(bytes) = tokio::fs::read(&path).await
        && let Some(kind) = image_type(&bytes)
    {
        return Some((bytes, kind));
    }
    if failed_recently(url) {
        return None;
    }
    let parsed = Url::parse(url).ok()?;
    let cancel = CancellationToken::new();
    let fetched = tokio::time::timeout(
        Duration::from_secs(45),
        kiwi.sources
            .artwork(&parsed, kiwi_source::MAX_ARTWORK_BYTES, &cancel),
    )
    .await;
    let Some((bytes, kind)) = (match fetched {
        Ok(Ok(bytes)) => image_type(&bytes).map(|kind| (bytes, kind)),
        Ok(Err(e)) => {
            tracing::debug!("artwork {url} unavailable: {e}");
            None
        }
        Err(_) => {
            // Possibly only waiting behind other artwork: not held against this URL.
            tracing::debug!("artwork {url} timed out");
            return None;
        }
    }) else {
        remember_failure(url);
        return None;
    };
    // Each fetch writes its own file, so requests for the same artwork at once can't mix
    // their bytes; the cache only ever holds whole files.
    let tmp = path.with_extension(format!("tmp-{}", hex::encode(random_bytes::<8>())));
    let saved = match tokio::fs::write(&tmp, &bytes).await {
        Ok(()) => tokio::fs::rename(&tmp, &path).await,
        Err(e) => Err(e),
    };
    if let Err(e) = saved {
        tracing::warn!("could not cache artwork: {e}");
        let _ = tokio::fs::remove_file(&tmp).await;
    }
    Some((bytes, kind))
}

/// Partial files (`<hash>.tmp-<random>`) older than this are left over from a crash.
const PARTIAL_RETENTION: Duration = Duration::from_secs(3600);

/// Deletes cached artwork older than [`RETENTION`] and leftover partial files.
pub async fn cleanup(dir: &Path, now: SystemTime) {
    let Ok(mut entries) = tokio::fs::read_dir(dir).await else {
        return;
    };
    while let Ok(Some(entry)) = entries.next_entry().await {
        let Ok(meta) = entry.metadata().await else {
            continue;
        };
        let age = meta
            .modified()
            .ok()
            .and_then(|m| now.duration_since(m).ok())
            .unwrap_or_default();
        let partial = entry.file_name().to_string_lossy().contains(".tmp");
        if meta.is_file() && (age > RETENTION || (partial && age > PARTIAL_RETENTION)) {
            let _ = tokio::fs::remove_file(entry.path()).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls_carry_a_tag_only_kiwi_can_make() {
        let url = "https://example.com/icons/a b.png?size=2&x=1";
        let client = client_url(url);
        let (tag, query) = client
            .strip_prefix("/api/art/")
            .unwrap()
            .split_once("?url=")
            .unwrap();
        assert_eq!(tag.len(), 64);
        let decoded: String = url::form_urlencoded::parse(format!("url={query}").as_bytes())
            .next()
            .unwrap()
            .1
            .into_owned();
        assert_eq!(decoded, url);
        assert!(verify(tag, url));
        assert!(!verify(tag, "https://example.com/other.png"));
        assert!(!verify(&"0".repeat(64), url));
        assert!(!verify("not hex", url));
    }

    #[test]
    fn the_key_is_kept_across_restarts() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("art.key");
        load_key(&path).unwrap();
        let stored = std::fs::read_to_string(&path).unwrap();
        assert_eq!(stored.len(), 64);
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        load_key(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap(), stored);
        // A damaged key is replaced.
        std::fs::write(&path, "nonsense").unwrap();
        load_key(&path).unwrap();
        assert_eq!(std::fs::read_to_string(&path).unwrap().len(), 64);
    }

    #[test]
    fn failures_are_remembered_for_a_while() {
        let url = "https://example.com/failures-are-remembered.png";
        assert!(!failed_recently(url));
        remember_failure(url);
        assert!(failed_recently(url));
    }

    #[test]
    fn recognizes_image_types() {
        assert_eq!(image_type(b"\x89PNG\r\n\x1a\n...."), Some("image/png"));
        assert_eq!(image_type(&[0xff, 0xd8, 0xff, 0xe0]), Some("image/jpeg"));
        assert_eq!(image_type(b"GIF89a.."), Some("image/gif"));
        assert_eq!(image_type(b"RIFF\0\0\0\0WEBPVP8 "), Some("image/webp"));
        assert_eq!(image_type(b"<html>"), None);
        assert_eq!(image_type(b"<svg xmlns="), None, "SVG can carry script");
    }

    #[tokio::test]
    async fn cleanup_deletes_old_artwork_only() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a"), b"x").unwrap();
        std::fs::write(dir.path().join("a.tmp-0123"), b"x").unwrap();
        cleanup(dir.path(), SystemTime::now()).await;
        assert!(dir.path().join("a").exists());
        assert!(
            dir.path().join("a.tmp-0123").exists(),
            "maybe still being written"
        );
        cleanup(dir.path(), SystemTime::now() + PARTIAL_RETENTION * 2).await;
        assert!(dir.path().join("a").exists());
        assert!(!dir.path().join("a.tmp-0123").exists());
        cleanup(dir.path(), SystemTime::now() + RETENTION * 2).await;
        assert!(!dir.path().join("a").exists());
    }
}
