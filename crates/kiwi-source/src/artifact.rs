use crate::{
    CancellationToken, DeviceProfile, Error, MAX_IPA_BYTES, Release, Result, SourceClient, Url,
    compare_numeric, platform_matches,
};
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::File,
    io::{BufReader, Read},
    path::Path,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};
use tokio::io::{AsyncWriteExt, BufWriter};

/// Coalesces network chunks and the ZIP reader's small reads without retaining the IPA.
const FILE_BUFFER_BYTES: usize = 256 * 1024;

#[derive(Debug, Default)]
pub struct DownloadProgress {
    pub received: AtomicU64,
    pub total: AtomicU64,
}
impl DownloadProgress {
    pub fn snapshot(&self) -> (u64, u64) {
        (
            self.received.load(Ordering::Relaxed),
            self.total.load(Ordering::Relaxed),
        )
    }
}

#[derive(Debug)]
pub struct ValidatedArtifact {
    // A private random filename owned by the app, removed on cancel/error/drop.
    file: tempfile::NamedTempFile,
    pub sha256: String,
    pub metadata: IpaMetadata,
}
impl ValidatedArtifact {
    pub fn path(&self) -> &Path {
        self.file.path()
    }
    /// Moves the file to `path` (same filesystem as the download directory), replacing it.
    pub fn persist(self, path: &Path) -> Result<()> {
        self.file
            .persist(path)
            .map(drop)
            .map_err(|e| Error::new("Download", e.error))
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpaMetadata {
    pub bundle_id: String,
    /// The Home Screen name (`CFBundleDisplayName`, else `CFBundleName`).
    pub name: Option<String>,
    pub version: String,
    pub build: Option<String>,
    pub min_os: Option<String>,
    pub platforms: Vec<String>,
    pub families: Vec<u64>,
}

/// Time allowed for a whole IPA download: 20 minutes plus one second per 128 KiB (about
/// 1 Mbit/s) of the declared size, or of the 4 GiB limit when no size is declared. Stalls
/// are caught much sooner by the client's per-read timeout.
fn download_time_limit(size: Option<u64>) -> Duration {
    let bytes = size.unwrap_or(MAX_IPA_BYTES).min(MAX_IPA_BYTES);
    Duration::from_secs(20 * 60 + bytes / (128 * 1024))
}

/// What an HTTP server says identifies a file's current content.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Validators {
    #[serde(default)]
    pub etag: Option<String>,
    #[serde(default)]
    pub last_modified: Option<String>,
    #[serde(default)]
    pub length: Option<u64>,
}
impl Validators {
    pub(crate) fn from_headers(headers: &reqwest::header::HeaderMap) -> Self {
        let text = |name| {
            headers
                .get(name)
                .and_then(|v| v.to_str().ok())
                .map(str::trim)
                .filter(|v| !v.is_empty())
                .map(str::to_owned)
        };
        Self {
            etag: text(reqwest::header::ETAG),
            last_modified: text(reqwest::header::LAST_MODIFIED),
            length: text(reqwest::header::CONTENT_LENGTH).and_then(|v| v.parse().ok()),
        }
    }
    pub fn is_empty(&self) -> bool {
        self.etag.is_none() && self.last_modified.is_none() && self.length.is_none()
    }
    /// Whether the file changed since `old`: a validator both have differs. Weak and
    /// strong forms of an ETag count as the same (compressing proxies weaken them).
    pub fn changed_from(&self, old: &Self) -> bool {
        fn differs<T: PartialEq>(a: &Option<T>, b: &Option<T>) -> bool {
            matches!((a, b), (Some(a), Some(b)) if a != b)
        }
        let strong = |e: &Option<String>| {
            e.as_deref()
                .map(|e| e.strip_prefix("W/").unwrap_or(e).to_owned())
        };
        differs(&strong(&self.etag), &strong(&old.etag))
            || differs(&self.last_modified, &old.last_modified)
            || differs(&self.length, &old.length)
    }
}

/// An IPA downloaded into a private temporary file, not yet inspected.
struct Fetched {
    file: tempfile::NamedTempFile,
    sha256: String,
    size: u64,
}

/// An IPA from a link (no source): downloaded and inspected, with what identifies it.
#[derive(Debug)]
pub struct LinkedIpa {
    pub artifact: ValidatedArtifact,
    pub size: u64,
    /// From the download response.
    pub validators: Validators,
}

impl SourceClient {
    /// Downloads into a private temporary file in `dir`, checks size and SHA-256, then
    /// inspects the archive and validates its metadata against the release. Device
    /// compatibility is checked only when `device` is given.
    pub async fn download(
        &self,
        release: &Release,
        bundle_id: &str,
        device: Option<&DeviceProfile>,
        dir: &Path,
        cancel: &CancellationToken,
        progress: &DownloadProgress,
    ) -> Result<ValidatedArtifact> {
        if let Some(reason) = device.and_then(|d| release.incompatibility(d)) {
            return Err(Error::new("IPA validation", reason));
        }
        let (fetched, _) = self
            .fetch_ipa(&release.download_url, release.size, dir, cancel, progress)
            .await?;
        let Fetched { file, sha256, size } = fetched;
        if let Some(expected) = release.size
            && size != expected
        {
            return Err(Error::new(
                "IPA validation",
                format!("Size mismatch: expected {expected} bytes, received {size}"),
            ));
        }
        if release
            .sha256
            .as_ref()
            .is_some_and(|expected| expected != &sha256)
        {
            return Err(Error::new(
                "IPA validation",
                format!(
                    "SHA-256 mismatch: expected {}, actual {sha256}",
                    release.sha256.as_deref().unwrap()
                ),
            ));
        }
        let expected = release.clone();
        let bundle = bundle_id.to_owned();
        let target = device.cloned();
        let cancellation = cancel.clone();
        tokio::task::spawn_blocking(move || {
            let metadata = inspect_ipa(file.path(), &cancellation)?;
            validate_metadata(&metadata, &expected, &bundle, target.as_ref())?;
            if cancellation.is_cancelled() {
                return Err(Error::new("IPA validation", "Cancelled"));
            }
            Ok(ValidatedArtifact {
                file,
                sha256,
                metadata,
            })
        })
        .await
        .map_err(|e| Error::new("IPA validation", e))?
    }

    /// Downloads and inspects the IPA at `url`, which no source describes: its metadata
    /// comes from the archive.
    pub async fn download_linked(
        &self,
        url: &Url,
        dir: &Path,
        cancel: &CancellationToken,
        progress: &DownloadProgress,
    ) -> Result<LinkedIpa> {
        let (fetched, validators) = self.fetch_ipa(url, None, dir, cancel, progress).await?;
        let Fetched { file, sha256, size } = fetched;
        let cancellation = cancel.clone();
        let artifact = tokio::task::spawn_blocking(move || {
            let metadata = inspect_ipa(file.path(), &cancellation)?;
            if !crate::valid_bundle_id(&metadata.bundle_id) {
                return Err(Error::new(
                    "IPA validation",
                    format!("Invalid bundle ID {:?}", metadata.bundle_id),
                ));
            }
            Ok(ValidatedArtifact {
                file,
                sha256,
                metadata,
            })
        })
        .await
        .map_err(|e| Error::new("IPA validation", e))??;
        Ok(LinkedIpa {
            artifact,
            size,
            validators,
        })
    }

    /// What identifies the file at `url` now, from a `HEAD` request. Empty when the
    /// server answers `HEAD` without any validator.
    pub async fn validators(&self, url: &Url, cancel: &CancellationToken) -> Result<Validators> {
        self.check_url(url)?;
        let cancelled = || Error::new("Update check", "Cancelled");
        let _permit = tokio::select! {
            _ = cancel.cancelled() => return Err(cancelled()),
            p = self.permits.acquire() => p.map_err(|e| Error::new("Update check", e))?,
        };
        let request = self
            .client
            .head(url.clone())
            .timeout(Duration::from_secs(30));
        let response = tokio::select! {
            _ = cancel.cancelled() => return Err(cancelled()),
            r = request.send() => r.map_err(|e| crate::network::request_error("Update check", e))?,
        };
        if !response.status().is_success() {
            return Err(Error::new(
                "Update check",
                format!("The IPA link answered HTTP {}.", response.status()),
            ));
        }
        Ok(Validators::from_headers(response.headers()))
    }

    /// Streams the IPA at `url` into a private temporary file in `dir`, hashing it.
    /// `size`: the declared size, for the time limit and progress.
    async fn fetch_ipa(
        &self,
        url: &Url,
        size: Option<u64>,
        dir: &Path,
        cancel: &CancellationToken,
        progress: &DownloadProgress,
    ) -> Result<(Fetched, Validators)> {
        self.check_url(url)?;
        let _permit = tokio::select! { _=cancel.cancelled()=>return Err(Error::new("Download","Cancelled")),p=self.permits.acquire()=>p.map_err(|e|Error::new("Download",e))? };
        let started = tokio::time::Instant::now();
        let too_slow = || {
            Error::new(
                "Download",
                "The download took too long. Retry on a faster connection.",
            )
        };
        let deadline = started + download_time_limit(size);
        let mut response = tokio::select! { _=cancel.cancelled()=>return Err(Error::new("Download","Cancelled")),_=tokio::time::sleep_until(deadline)=>return Err(too_slow()),r=self.client.get(url.clone()).send()=>r.map_err(|e|crate::network::request_error("Download",e))? };
        if !response.status().is_success() {
            return Err(Error::new(
                "Download",
                format!(
                    "HTTP {}. Retry, or check the release with its publisher.",
                    response.status()
                ),
            ));
        }
        if crate::network::is_html(&response) {
            return Err(Error::new(
                "Download",
                "Server returned HTML instead of an IPA (login page, expired link, or rate limit)",
            ));
        }
        let validators = Validators::from_headers(response.headers());
        let length = response.content_length();
        if length.is_some_and(|n| n > MAX_IPA_BYTES) || size.is_some_and(|n| n > MAX_IPA_BYTES) {
            return Err(Error::new("Download", "IPA exceeds 4 GiB limit"));
        }
        progress
            .total
            .store(length.or(size).unwrap_or(0), Ordering::Relaxed);
        let deadline = started + download_time_limit(length.or(size));
        tokio::fs::create_dir_all(dir)
            .await
            .map_err(|e| Error::new("Download", e))?;
        let directory = dir.to_owned();
        let temp = tokio::task::spawn_blocking(move || {
            tempfile::Builder::new()
                .prefix("source-")
                .suffix(".ipa")
                .tempfile_in(directory)
        })
        .await
        .map_err(|e| Error::new("Download", e))?
        .map_err(|e| Error::new("Download", e))?;
        let mut output = BufWriter::with_capacity(
            FILE_BUFFER_BYTES,
            tokio::fs::File::from_std(temp.reopen().map_err(|e| Error::new("Download", e))?),
        );
        let mut hash = Sha256::new();
        let mut size = 0u64;
        loop {
            let chunk = tokio::select! { _=cancel.cancelled()=>return Err(Error::new("Download","Cancelled")),_=tokio::time::sleep_until(deadline)=>return Err(too_slow()),r=response.chunk()=>r.map_err(|e|crate::network::request_error("Download",e))? };
            let Some(chunk) = chunk else {
                break;
            };
            size += chunk.len() as u64;
            if size > MAX_IPA_BYTES {
                return Err(Error::new("Download", "IPA exceeds 4 GiB limit"));
            }
            // Hashing each network chunk is bounded work. Full archive work runs on a worker.
            hash.update(&chunk);
            output
                .write_all(&chunk)
                .await
                .map_err(|e| Error::new("Download", e))?;
            progress.received.store(size, Ordering::Relaxed);
        }
        output
            .flush()
            .await
            .map_err(|e| Error::new("Download", e))?;
        // On disk before the caller renames it into the cache, so a power loss cannot
        // leave a truncated file under the artifact's hash.
        output
            .get_ref()
            .sync_all()
            .await
            .map_err(|e| Error::new("Download", e))?;
        drop(output);
        if size == 0 || length.is_some_and(|n| n != size) {
            return Err(Error::new(
                "Download",
                format!("Incomplete response: received {size} bytes, expected {length:?}"),
            ));
        }
        Ok((
            Fetched {
                file: temp,
                sha256: hex::encode(hash.finalize()),
                size,
            },
            validators,
        ))
    }
}

/// Largest symlink target read from an archive.
const MAX_LINK_TARGET: u64 = 4096;

/// Path components of an archive entry name (Unix semantics, as the signer extracts it):
/// `None` for absolute paths, `..`, NUL bytes or an empty path. `.` and empty components
/// are skipped.
fn entry_components(name: &str) -> Option<Vec<&str>> {
    if name.starts_with('/') || name.contains('\0') {
        return None;
    }
    let parts: Vec<&str> = name
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    (!parts.is_empty() && !parts.contains(&"..")).then_some(parts)
}

/// Whether a symlink at `link` pointing to `target` leaves the archive root (lexically).
fn link_escapes(link: &[&str], target: &str) -> bool {
    if target.is_empty() || target.starts_with('/') || target.contains('\0') {
        return true;
    }
    let mut stack: Vec<&str> = link[..link.len().saturating_sub(1)].to_vec();
    for part in target.split('/') {
        match part {
            "" | "." => {}
            ".." => {
                if stack.pop().is_none() {
                    return true;
                }
            }
            part => stack.push(part),
        }
    }
    false
}

/// Before extraction: reject traversal (absolute paths, `..`), duplicate or case-colliding
/// paths, symlinks leaving the archive and entries below a symlink, zip bombs, multiple apps,
/// and corrupt data. Reading every entry also checks ZIP CRCs, including truncated payloads.
/// Symlinks inside the archive (framework `Versions/Current` links) are allowed.
pub fn inspect_ipa(path: &Path, cancel: &CancellationToken) -> Result<IpaMetadata> {
    let file = File::open(path).map_err(|e| Error::new("IPA validation", e))?;
    let mut zip = zip::ZipArchive::new(BufReader::with_capacity(FILE_BUFFER_BYTES, file))
        .map_err(|e| Error::new("IPA validation", format!("Invalid IPA ZIP archive: {e}")))?;
    if zip.len() > 100_000 {
        return Err(Error::new("IPA validation", "Archive has too many entries"));
    }
    let mut total = 0u64;
    let mut names = HashSet::new();
    let mut links = HashSet::new();
    let mut paths = Vec::with_capacity(zip.len());
    let mut apps = HashSet::new();
    let mut main = None;
    let mut buffer = [0u8; 64 * 1024];
    for i in 0..zip.len() {
        if cancel.is_cancelled() {
            return Err(Error::new("IPA validation", "Cancelled"));
        }
        let mut entry = zip
            .by_index(i)
            .map_err(|e| Error::new("IPA validation", e))?;
        let name = entry
            .name()
            .map_err(|e| Error::new("IPA validation", e))?
            .into_owned();
        let unsafe_path = || {
            Error::new(
                "IPA validation",
                format!("Unsafe or duplicate archive path: {name}"),
            )
        };
        let parts = entry_components(&name).ok_or_else(unsafe_path)?;
        // Preserve the validated identity on case-insensitive filesystems (macOS).
        let key = parts.join("/").to_lowercase();
        if !names.insert(key.clone()) {
            return Err(unsafe_path());
        }
        let is_link = entry.is_symlink();
        if is_link {
            if entry.size() > MAX_LINK_TARGET {
                return Err(unsafe_path());
            }
            links.insert(key.clone());
        }
        paths.push(key);
        total = total
            .checked_add(entry.size())
            .ok_or_else(|| Error::new("IPA validation", "Archive size overflow"))?;
        if total > 8 * 1024 * 1024 * 1024 || entry.size() > 2 * 1024 * 1024 * 1024 {
            return Err(Error::new(
                "IPA validation",
                "Uncompressed IPA exceeds safety limit",
            ));
        }
        if parts.len() >= 2 && parts[0] == "Payload" && parts[1].ends_with(".app") {
            apps.insert(parts[1].to_owned());
        }
        if apps.len() > 1 {
            return Err(Error::new(
                "IPA validation",
                "Archive contains multiple main Payload apps",
            ));
        }
        let is_info = parts.len() == 3
            && parts[0] == "Payload"
            && parts[1].ends_with(".app")
            && parts[2] == "Info.plist"
            && !is_link;
        if is_info && (main.is_some() || entry.size() > 2 * 1024 * 1024) {
            return Err(Error::new(
                "IPA validation",
                "Expected exactly one main app Info.plist, at most 2 MiB",
            ));
        }
        let mut plist = vec![];
        let mut read = 0u64;
        loop {
            if cancel.is_cancelled() {
                return Err(Error::new("IPA validation", "Cancelled"));
            }
            let n = entry
                .read(&mut buffer)
                .map_err(|e| Error::new("IPA validation", format!("Corrupt entry {name}: {e}")))?;
            if n == 0 {
                break;
            }
            read += n as u64;
            if read > entry.size() {
                return Err(Error::new(
                    "IPA validation",
                    "Archive entry exceeds declared size",
                ));
            }
            if is_info || is_link {
                plist.extend_from_slice(&buffer[..n]);
            }
        }
        if read != entry.size() {
            return Err(Error::new(
                "IPA validation",
                format!("Incomplete archive entry {name}"),
            ));
        }
        if is_link {
            let target = String::from_utf8_lossy(&plist);
            if link_escapes(&parts, &target) {
                return Err(Error::new(
                    "IPA validation",
                    format!("Symbolic link points outside the archive: {name} -> {target}"),
                ));
            }
        }
        if is_info {
            main = Some(plist);
        }
    }
    // Nothing may be extracted through a symlink.
    if let Some(inside) = paths
        .iter()
        .find(|p| p.match_indices('/').any(|(i, _)| links.contains(&p[..i])))
    {
        return Err(Error::new(
            "IPA validation",
            format!("Archive entry inside a symbolic link: {inside}"),
        ));
    }
    let main =
        main.ok_or_else(|| Error::new("IPA validation", "Missing Payload/*.app/Info.plist"))?;
    check_plist(&main).map_err(|e| Error::new("IPA validation", format!("Info.plist: {e}")))?;
    let dict: plist::Dictionary =
        plist::from_bytes(&main).map_err(|e| Error::new("IPA validation", e))?;
    let string = |key| {
        dict.get(key)
            .and_then(plist::Value::as_string)
            .map(str::to_owned)
    };
    let bundle_id = string("CFBundleIdentifier")
        .ok_or_else(|| Error::new("IPA validation", "Missing CFBundleIdentifier"))?;
    let version = string("CFBundleShortVersionString")
        .ok_or_else(|| Error::new("IPA validation", "Missing CFBundleShortVersionString"))?;
    let platforms = dict
        .get("CFBundleSupportedPlatforms")
        .and_then(plist::Value::as_array)
        .map(|v| {
            v.iter()
                .filter_map(plist::Value::as_string)
                .map(str::to_owned)
                .collect()
        })
        .unwrap_or_default();
    let families = dict
        .get("UIDeviceFamily")
        .and_then(plist::Value::as_array)
        .map(|v| {
            v.iter()
                .filter_map(plist::Value::as_unsigned_integer)
                .collect()
        })
        .unwrap_or_default();
    Ok(IpaMetadata {
        bundle_id,
        name: string("CFBundleDisplayName")
            .or_else(|| string("CFBundleName"))
            .map(|n| n.trim().to_owned())
            .filter(|n| !n.is_empty()),
        version,
        build: string("CFBundleVersion"),
        min_os: string("MinimumOSVersion"),
        platforms,
        families,
    })
}

/// Deepest array/dictionary nesting accepted in an IPA's plist; deserializing recurses per
/// level, so a hostile file could otherwise overflow the stack and abort the server.
const MAX_PLIST_DEPTH: usize = 64;
/// Most values accepted in one plist (binary plists can reference one object many times).
const MAX_PLIST_VALUES: usize = 1_000_000;

/// Checks a plist's nesting and size with the (non-recursive) event reader before it is
/// deserialized.
pub(crate) fn check_plist(bytes: &[u8]) -> std::result::Result<(), String> {
    use plist::stream::{Event, Reader};
    let mut depth = 0usize;
    for (count, event) in Reader::new(std::io::Cursor::new(bytes)).enumerate() {
        if count >= MAX_PLIST_VALUES {
            return Err(format!("more than {MAX_PLIST_VALUES} values"));
        }
        match event.map_err(|e| e.to_string())? {
            Event::StartArray(_) | Event::StartDictionary(_) => {
                depth += 1;
                if depth > MAX_PLIST_DEPTH {
                    return Err(format!("nested more than {MAX_PLIST_DEPTH} levels deep"));
                }
            }
            Event::EndCollection => depth = depth.saturating_sub(1),
            _ => {}
        }
    }
    Ok(())
}

/// Checks the IPA's identity against the release, and its requirements against `device`
/// when one is given.
pub fn validate_metadata(
    actual: &IpaMetadata,
    expected: &Release,
    bundle: &str,
    device: Option<&DeviceProfile>,
) -> Result<()> {
    if actual.bundle_id != bundle {
        return Err(Error::new(
            "IPA validation",
            format!(
                "Bundle ID mismatch: expected {bundle}, actual {}",
                actual.bundle_id
            ),
        ));
    }
    if actual.version != expected.version {
        return Err(Error::new(
            "IPA validation",
            format!(
                "Version mismatch: expected {}, actual {}. Ask the publisher to correct the feed.",
                expected.version, actual.version
            ),
        ));
    }
    if expected.build.is_some() && expected.build != actual.build {
        return Err(Error::new(
            "IPA validation",
            format!(
                "Build mismatch: expected {:?}, actual {:?}",
                expected.build, actual.build
            ),
        ));
    }
    let Some(device) = device else {
        return Ok(());
    };
    if actual.platforms.is_empty()
        || !actual
            .platforms
            .iter()
            .any(|p| platform_matches(p, &device.platform))
    {
        return Err(Error::new(
            "IPA validation",
            format!(
                "IPA platforms {:?} do not support selected platform {}",
                actual.platforms, device.platform
            ),
        ));
    }
    if !actual.families.is_empty() && !actual.families.contains(&device.family) {
        return Err(Error::new(
            "IPA validation",
            format!(
                "IPA device families {:?} do not support selected family {}",
                actual.families, device.family
            ),
        ));
    }
    if let Some(min) = &actual.min_os {
        match compare_numeric(&device.os_version, min) {
            Some(std::cmp::Ordering::Less) | None => {
                return Err(Error::new(
                    "IPA validation",
                    format!(
                        "IPA requires OS {min}+, selected device has {}",
                        device.os_version
                    ),
                ));
            }
            _ => {}
        }
    }
    if let Some(reason) = expected.incompatibility(device) {
        return Err(Error::new("IPA validation", reason));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_time_scales_with_size() {
        let minutes = |size| download_time_limit(size).as_secs() / 60;
        assert_eq!(minutes(Some(1_000_000)), 20);
        // 150 MB: 20 minutes plus about 19 at 1 Mbit/s.
        assert_eq!(minutes(Some(150_000_000)), 39);
        // Unknown or oversized: the 4 GiB limit, about 9 hours.
        assert_eq!(minutes(None), 20 + 32_768 / 60);
        assert_eq!(
            download_time_limit(Some(u64::MAX)),
            download_time_limit(None)
        );
    }

    #[test]
    fn plists_are_checked_before_deserializing() {
        let nested = |n: usize| {
            format!(
                r#"<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict><key>a</key>{}<string>x</string>{}</dict></plist>"#,
                "<array>".repeat(n),
                "</array>".repeat(n)
            )
        };
        assert_eq!(check_plist(nested(63).as_bytes()), Ok(()));
        let err = check_plist(nested(100_000).as_bytes()).unwrap_err();
        assert!(err.contains("nested more than 64"), "{err}");
        assert!(check_plist(b"<plist><dict><key>a</key></plist>").is_err());
        // A binary plist whose arrays reference the same child twice per level: 2^40
        // values when expanded.
        let mut objects = vec![vec![0x10u8, 0x01]]; // integer 1
        for i in 0..40u8 {
            objects.push(vec![0xa2, i, i]);
        }
        assert_eq!(check_plist(&binary_plist(&objects[..4])), Ok(()));
        let err = check_plist(&binary_plist(&objects)).unwrap_err();
        assert!(err.contains("more than 1000000 values"), "{err}");
    }

    /// A binary plist of `objects` (1-byte references); the last object is the root.
    fn binary_plist(objects: &[Vec<u8>]) -> Vec<u8> {
        let mut out = b"bplist00".to_vec();
        let mut offsets = Vec::new();
        for object in objects {
            offsets.push(out.len() as u16);
            out.extend_from_slice(object);
        }
        let table = out.len() as u64;
        for offset in &offsets {
            out.extend_from_slice(&offset.to_be_bytes());
        }
        out.extend_from_slice(&[0; 6]);
        out.extend_from_slice(&[2, 1]); // offset size, reference size
        out.extend_from_slice(&(objects.len() as u64).to_be_bytes());
        out.extend_from_slice(&(objects.len() as u64 - 1).to_be_bytes());
        out.extend_from_slice(&table.to_be_bytes());
        out
    }
}
