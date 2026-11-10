//! Over-the-air install endpoints: manifest, IPA or APK (with Range) and icon.

use std::{
    collections::HashMap,
    io,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{Arc, Mutex},
    task::{Context, Poll, ready},
    time::{Duration, Instant},
};

use axum::{
    body::{Body, Bytes},
    extract::{Path as UrlPath, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use futures_util::Stream;
use plist::{Dictionary, Value};
use sha2::{Digest, Sha256};
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tokio_util::io::ReaderStream;

use crate::{
    Kiwi,
    db::{AttemptRow, BuildRow, InstallRow},
    devices::record_user_agent,
};

/// `itms-services` link for a token (api.md).
pub fn install_url(public_url: &str, token: &str) -> String {
    let manifest = format!(
        "{}/ota/{token}/manifest.plist",
        public_url.trim_end_matches('/')
    );
    let encoded: String = url::form_urlencoded::byte_serialize(manifest.as_bytes()).collect();
    format!("itms-services://?action=download-manifest&url={encoded}")
}

/// The download link of an Android build for a token (api.md). Android has no install
/// manifest: the browser downloads the APK and the phone's installer opens it.
pub fn apk_url(public_url: &str, token: &str) -> String {
    format!("{}/ota/{token}/app.apk", public_url.trim_end_matches('/'))
}

/// The OTA manifest plist (XML).
pub fn manifest(public_url: &str, token: &str, build: &BuildRow, has_icon: bool) -> Vec<u8> {
    let base = format!("{}/ota/{token}", public_url.trim_end_matches('/'));
    let asset = |kind: &str, url: String| {
        let mut d = Dictionary::new();
        d.insert("kind".into(), kind.into());
        d.insert("url".into(), url.into());
        Value::Dictionary(d)
    };
    let mut assets = vec![asset("software-package", format!("{base}/app.ipa"))];
    if has_icon {
        assets.push(asset("display-image", format!("{base}/icon.png")));
        assets.push(asset("full-size-image", format!("{base}/icon.png")));
    }
    let mut metadata = Dictionary::new();
    metadata.insert(
        "bundle-identifier".into(),
        build.signed_bundle_id.clone().into(),
    );
    metadata.insert("bundle-version".into(), build.version.clone().into());
    metadata.insert("kind".into(), "software".into());
    metadata.insert("title".into(), build.app_name.clone().into());
    let mut item = Dictionary::new();
    item.insert("assets".into(), Value::Array(assets));
    item.insert("metadata".into(), Value::Dictionary(metadata));
    let mut root = Dictionary::new();
    root.insert("items".into(), Value::Array(vec![Value::Dictionary(item)]));
    let mut out = Vec::new();
    plist::to_writer_xml(&mut out, &Value::Dictionary(root)).expect("in-memory plist");
    out
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByteRange {
    Full,
    /// Inclusive byte positions.
    Partial {
        start: u64,
        end: u64,
    },
    Unsatisfiable,
}

/// A single `bytes=` range (RFC 9110). Invalid or multi-range headers are ignored (full
/// response); ranges starting past the end are unsatisfiable.
pub fn parse_range(header: Option<&str>, len: u64) -> ByteRange {
    let Some(spec) = header.and_then(|h| h.trim().strip_prefix("bytes=")) else {
        return ByteRange::Full;
    };
    if spec.contains(',') {
        return ByteRange::Full;
    }
    let Some((first, last)) = spec.trim().split_once('-') else {
        return ByteRange::Full;
    };
    let number = |s: &str| {
        (!s.is_empty() && s.bytes().all(|b| b.is_ascii_digit()))
            .then(|| s.parse::<u64>().ok())
            .flatten()
    };
    match (first.trim(), last.trim()) {
        ("", suffix) => match number(suffix) {
            Some(0) => ByteRange::Unsatisfiable,
            Some(_) if len == 0 => ByteRange::Unsatisfiable,
            Some(n) => ByteRange::Partial {
                start: len.saturating_sub(n),
                end: len - 1,
            },
            None => ByteRange::Full,
        },
        (start, end) => {
            let Some(start) = number(start) else {
                return ByteRange::Full;
            };
            let end = match end {
                "" => None,
                e => match number(e) {
                    Some(e) if e >= start => Some(e),
                    _ => return ByteRange::Full,
                },
            };
            if start >= len {
                ByteRange::Unsatisfiable
            } else {
                ByteRange::Partial {
                    start,
                    end: end.map_or(len - 1, |e| e.min(len - 1)),
                }
            }
        }
    }
}

/// Adds the inclusive range `start..=end` to sorted, disjoint `ranges`, merging overlapping
/// and adjacent ones.
pub fn merge_range(ranges: &mut Vec<(u64, u64)>, start: u64, end: u64) {
    ranges.push((start, end));
    ranges.sort_unstable();
    let mut merged: Vec<(u64, u64)> = Vec::with_capacity(ranges.len());
    for &(s, e) in ranges.iter() {
        match merged.last_mut() {
            Some(last) if s <= last.1.saturating_add(1) => last.1 = last.1.max(e),
            _ => merged.push((s, e)),
        }
    }
    *ranges = merged;
}

/// How long an install link (its token) works. Kiwi hands out a new one once the old one is
/// [`TOKEN_RENEWAL`] old, so a link the web app shows has hours left.
pub const TOKEN_LIFETIME: chrono::Duration = chrono::Duration::hours(24);
pub const TOKEN_RENEWAL: chrono::Duration = chrono::Duration::hours(12);
/// A token the installer used this recently keeps working past [`TOKEN_LIFETIME`] (an
/// install in progress), up to [`TOKEN_LIMIT`].
pub const TOKEN_ACTIVE: chrono::Duration = chrono::Duration::hours(1);
pub const TOKEN_LIMIT: chrono::Duration = chrono::Duration::hours(48);
/// A replaced token keeps leading to its build for this long: a page or app that still shows
/// the old link, and a download that started with it, go on working.
const RETIRED_TTL: Duration = Duration::from_secs(2 * 60 * 60);

/// Whether an attempt's token still works at `now` ([`TOKEN_LIFETIME`], [`TOKEN_ACTIVE`]).
pub fn token_alive(attempt: &AttemptRow, now: chrono::DateTime<Utc>) -> bool {
    let age = now - attempt.created_at;
    let active = attempt
        .manifest_at
        .max(attempt.download_started_at)
        .is_some_and(|at| now - at < TOKEN_ACTIVE);
    age < TOKEN_LIFETIME || (active && age < TOKEN_LIMIT)
}

/// Whether to hand out a new token instead of this attempt's: once it is
/// [`TOKEN_RENEWAL`] old, unless the installer is using it, and always once it is past
/// [`TOKEN_LIFETIME`] (the old one keeps working for a while, [`RetiredTokens`]).
pub fn renew_token(attempt: &AttemptRow, now: chrono::DateTime<Utc>) -> bool {
    let age = now - attempt.created_at;
    let active = attempt
        .manifest_at
        .max(attempt.download_started_at)
        .is_some_and(|at| now - at < TOKEN_ACTIVE);
    age >= TOKEN_LIFETIME || (age >= TOKEN_RENEWAL && !active)
}

/// Replaced tokens and the token that replaced them, in memory ([`RETIRED_TTL`]).
#[derive(Default)]
pub struct RetiredTokens(Mutex<HashMap<String, (String, Instant)>>);

impl RetiredTokens {
    pub fn add(&self, old: String, new: String) {
        let mut map = self.0.lock().unwrap_or_else(|e| e.into_inner());
        map.retain(|_, (_, at)| at.elapsed() < RETIRED_TTL);
        map.insert(old, (new, Instant::now()));
    }

    fn current(&self, old: &str) -> Option<String> {
        let map = self.0.lock().unwrap_or_else(|e| e.into_inner());
        map.get(old)
            .filter(|(_, at)| at.elapsed() < RETIRED_TTL)
            .map(|(new, _)| new.clone())
    }
}

/// Forgotten when no range was served for a token for this long.
const SERVED_TTL: Duration = Duration::from_secs(60 * 60);

struct Served {
    len: u64,
    ranges: Vec<(u64, u64)>,
    updated: Instant,
}

/// The IPA byte ranges fully served per install token, in memory. A download made of
/// `Range` requests completes once its ranges cover the whole file.
#[derive(Default)]
pub struct ServedRanges(Mutex<HashMap<String, Served>>);

impl ServedRanges {
    /// Records that bytes `start..=end` of a `len`-byte IPA were served for `token`; true
    /// when the token's ranges now cover the whole file (its record is then cleared).
    pub fn add(&self, token: &str, len: u64, start: u64, end: u64) -> bool {
        let mut map = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let now = Instant::now();
        map.retain(|_, s| now.duration_since(s.updated) < SERVED_TTL);
        let served = map.entry(token.to_owned()).or_insert_with(|| Served {
            len,
            ranges: Vec::new(),
            updated: now,
        });
        if served.len != len {
            served.len = len;
            served.ranges.clear();
        }
        served.updated = now;
        merge_range(&mut served.ranges, start, end);
        let complete = len > 0 && served.ranges == [(0, len - 1)];
        if complete {
            map.remove(token);
        }
        complete
    }
}

pub fn icon_path(icons_dir: &Path, url: &str) -> PathBuf {
    icons_dir.join(format!(
        "{}.png",
        hex::encode(Sha256::digest(url.as_bytes()))
    ))
}

/// Streams part of a file; runs `on_complete` once every byte has been handed to the
/// server (the stream ends or is dropped after its last chunk).
struct TrackedStream {
    inner: ReaderStream<tokio::io::Take<tokio::fs::File>>,
    remaining: u64,
    on_complete: Option<Box<dyn FnOnce() + Send>>,
}

impl TrackedStream {
    fn finish(&mut self) {
        if self.remaining == 0
            && let Some(f) = self.on_complete.take()
        {
            f();
        }
    }
}

impl Stream for TrackedStream {
    type Item = io::Result<Bytes>;
    fn poll_next(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Option<Self::Item>> {
        let item = ready!(Pin::new(&mut self.inner).poll_next(cx));
        match &item {
            Some(Ok(chunk)) => self.remaining = self.remaining.saturating_sub(chunk.len() as u64),
            Some(Err(_)) => self.on_complete = None,
            None if self.remaining > 0 => {
                self.on_complete = None;
                self.remaining = 0;
                return Poll::Ready(Some(Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "file shorter than expected",
                ))));
            }
            None => self.finish(),
        }
        Poll::Ready(item)
    }
}

impl Drop for TrackedStream {
    fn drop(&mut self) {
        self.finish();
    }
}

fn valid_token(token: &str) -> bool {
    token.len() == 22
        && token
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn not_found() -> Response {
    (StatusCode::NOT_FOUND, "Not found").into_response()
}

fn user_agent(headers: &HeaderMap) -> Option<String> {
    headers
        .get(header::USER_AGENT)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.chars().take(512).collect())
}

/// The attempt and its ready build. A replaced token leads to its attempt for a while
/// ([`RetiredTokens`]); `attempt.token` is the current one either way.
fn lookup(kiwi: &Kiwi, token: &str) -> Option<(AttemptRow, BuildRow)> {
    if !valid_token(token) {
        return None;
    }
    let attempt = match kiwi.db.attempt(token).ok()? {
        Some(attempt) => attempt,
        None => kiwi.db.attempt(&kiwi.retired.current(token)?).ok()??,
    };
    let now = Utc::now();
    if !token_alive(&attempt, now) {
        return None;
    }
    let build = kiwi.db.build(&attempt.build_id).ok()??;
    build.usable(now).then_some((attempt, build))
}

fn observe_device(kiwi: &Kiwi, device_id: &str, ua: Option<&str>) {
    if let Ok(Some(mut device)) = kiwi.db.device(device_id)
        && let Err(e) = record_user_agent(&kiwi.db, &mut device, ua)
    {
        tracing::warn!("could not save device {device_id}: {e}");
    }
}

pub async fn manifest_handler(
    State(kiwi): State<Arc<Kiwi>>,
    UrlPath(token): UrlPath<String>,
    headers: HeaderMap,
) -> Response {
    let Some((attempt, build)) = lookup(&kiwi, &token) else {
        return not_found();
    };
    // An Android build is a plain download; there is no manifest to install it from.
    if crate::builds::android(&build.release) {
        return not_found();
    }
    let token = attempt.token.clone();
    let ua = user_agent(&headers);
    if let Err(e) = kiwi.db.attempt_manifest(&token, ua.as_deref(), Utc::now()) {
        tracing::warn!("could not record manifest fetch: {e}");
    }
    observe_device(&kiwi, &attempt.device_id, ua.as_deref());
    let has_icon = build
        .icon_url
        .as_ref()
        .is_some_and(|u| icon_path(&kiwi.paths.icons, u).is_file());
    tracing::info!(build = %build.id, device = %attempt.device_id, "manifest fetched");
    let body = manifest(&kiwi.public_url(&headers), &token, &build, has_icon);
    (
        [
            (header::CONTENT_TYPE, "application/xml"),
            (header::CACHE_CONTROL, "no-store"),
        ],
        body,
    )
        .into_response()
}

pub async fn ipa_handler(
    State(kiwi): State<Arc<Kiwi>>,
    UrlPath(token): UrlPath<String>,
    method: Method,
    headers: HeaderMap,
) -> Response {
    package(kiwi, token, method, headers, false).await
}

pub async fn apk_handler(
    State(kiwi): State<Arc<Kiwi>>,
    UrlPath(token): UrlPath<String>,
    method: Method,
    headers: HeaderMap,
) -> Response {
    package(kiwi, token, method, headers, true).await
}

/// The name a browser saves an Android build under, e.g. `YouTube-21.30.216.apk`.
fn apk_file_name(build: &BuildRow) -> String {
    let safe = |text: &str| -> String {
        text.chars()
            .filter(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '_'))
            .take(48)
            .collect()
    };
    let name = Some(safe(&build.app_name))
        .filter(|n| !n.trim_matches('.').is_empty())
        .unwrap_or_else(|| "App".into());
    match safe(&build.version) {
        version if version.is_empty() => format!("{name}.apk"),
        version => format!("{name}-{version}.apk"),
    }
}

/// Serves a build's app file: the signed IPA to the iOS installer, or the APK to an
/// Android browser. Each has its own path, so neither is ever served as the other.
async fn package(
    kiwi: Arc<Kiwi>,
    token: String,
    method: Method,
    headers: HeaderMap,
    android: bool,
) -> Response {
    let Some((attempt, build)) = lookup(&kiwi, &token) else {
        return not_found();
    };
    if crate::builds::android(&build.release) != android {
        return not_found();
    }
    let token = attempt.token.clone();
    let Some(path) = build.ipa_path.as_ref().map(PathBuf::from) else {
        return not_found();
    };
    let Ok(mut file) = tokio::fs::File::open(&path).await else {
        return not_found();
    };
    let Ok(len) = file.metadata().await.map(|m| m.len()) else {
        return not_found();
    };
    let (status, start, end) = match requested_range(&headers, len) {
        Ok(range) => range,
        Err(response) => return *response,
    };
    let content_type = if android {
        "application/vnd.android.package-archive"
    } else {
        "application/octet-stream"
    };
    let file_name = android.then(|| apk_file_name(&build));
    let response = file_head(status, start, end, len, content_type, file_name.as_deref());
    if method == Method::HEAD {
        return response.body(Body::empty()).unwrap_or_else(|_| not_found());
    }
    if start > 0 && file.seek(io::SeekFrom::Start(start)).await.is_err() {
        return not_found();
    }
    if let Err(e) = kiwi.db.attempt_download_started(&token, Utc::now()) {
        tracing::warn!("could not record download start: {e}");
    }
    tracing::info!(build = %build.id, device = %attempt.device_id, start, end, len, "app download started");
    // A full response completes the download once its last byte is written; ranges only
    // once those served for this attempt cover the whole file (not e.g. just the zip tail).
    // Ranges count per build and device, not per token, which may be renewed meanwhile.
    let full = status == StatusCode::OK;
    let served_key = format!("{}/{}", attempt.build_id, attempt.device_id);
    let on_complete: Box<dyn FnOnce() + Send> = {
        let kiwi = kiwi.clone();
        let ua = user_agent(&headers);
        Box::new(move || {
            if full || kiwi.served.add(&served_key, len, start, end) {
                complete_download(&kiwi, &token, ua.as_deref());
            }
        })
    };
    response
        .body(file_body(file, start, end, Some(on_complete)))
        .unwrap_or_else(|_| not_found())
}

/// The status and inclusive byte positions a request's `Range` asks for in a `len`-byte
/// file, or the response for a range that can't be served.
fn requested_range(headers: &HeaderMap, len: u64) -> Result<(StatusCode, u64, u64), Box<Response>> {
    let range = parse_range(
        headers.get(header::RANGE).and_then(|v| v.to_str().ok()),
        len,
    );
    match range {
        ByteRange::Unsatisfiable => Err(Box::new(
            (
                StatusCode::RANGE_NOT_SATISFIABLE,
                [(header::CONTENT_RANGE, format!("bytes */{len}"))],
            )
                .into_response(),
        )),
        ByteRange::Full if len == 0 => Err(Box::new(not_found())),
        ByteRange::Full => Ok((StatusCode::OK, 0, len - 1)),
        ByteRange::Partial { start, end } => Ok((StatusCode::PARTIAL_CONTENT, start, end)),
    }
}

/// The headers of a file download; `file_name` makes it an attachment.
fn file_head(
    status: StatusCode,
    start: u64,
    end: u64,
    len: u64,
    content_type: &str,
    file_name: Option<&str>,
) -> axum::http::response::Builder {
    let mut response = Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::CONTENT_LENGTH, end - start + 1)
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CACHE_CONTROL, "no-store");
    if status == StatusCode::PARTIAL_CONTENT {
        response = response.header(header::CONTENT_RANGE, format!("bytes {start}-{end}/{len}"));
    }
    if let Some(name) = file_name {
        response = response.header(
            header::CONTENT_DISPOSITION,
            format!("attachment; filename=\"{name}\""),
        );
    }
    response
}

/// Bytes `start..=end` of a file positioned at `start`; `on_complete` runs once the last
/// one was handed to the server.
fn file_body(
    file: tokio::fs::File,
    start: u64,
    end: u64,
    on_complete: Option<Box<dyn FnOnce() + Send>>,
) -> Body {
    let count = end - start + 1;
    Body::from_stream(TrackedStream {
        inner: ReaderStream::with_capacity(file.take(count), 256 * 1024),
        remaining: count,
        on_complete,
    })
}

/// Serves a whole file, or the part a `Range` asks for, as an attachment named `file_name`;
/// 404 unless the file has `expected_len` bytes.
pub async fn send_file(
    path: &Path,
    expected_len: u64,
    method: &Method,
    headers: &HeaderMap,
    content_type: &str,
    file_name: &str,
) -> Response {
    let Ok(mut file) = tokio::fs::File::open(path).await else {
        return not_found();
    };
    let len = match file.metadata().await {
        Ok(m) if m.is_file() && m.len() == expected_len => m.len(),
        _ => return not_found(),
    };
    let (status, start, end) = match requested_range(headers, len) {
        Ok(range) => range,
        Err(response) => return *response,
    };
    let response = file_head(status, start, end, len, content_type, Some(file_name));
    if *method == Method::HEAD {
        return response.body(Body::empty()).unwrap_or_else(|_| not_found());
    }
    if start > 0 && file.seek(io::SeekFrom::Start(start)).await.is_err() {
        return not_found();
    }
    response
        .body(file_body(file, start, end, None))
        .unwrap_or_else(|_| not_found())
}

/// The whole IPA or APK was sent: the install is recorded for the device. `token` may have
/// been renewed while the file was sent; then its replacement is the attempt's.
fn complete_download(kiwi: &Kiwi, token: &str, ua: Option<&str>) {
    let now = Utc::now();
    let result = (|| -> rusqlite::Result<()> {
        let token = match kiwi.db.attempt(token)? {
            Some(_) => token.to_owned(),
            None => match kiwi.retired.current(token) {
                Some(current) => current,
                None => return Ok(()),
            },
        };
        let token = token.as_str();
        kiwi.db.attempt_download_completed(token, ua, now)?;
        let Some(attempt) = kiwi.db.attempt(token)? else {
            return Ok(());
        };
        let Some(build) = kiwi.db.build(&attempt.build_id)? else {
            return Ok(());
        };
        let developer = kiwi
            .catalog
            .app(&build.source_id, &build.bundle_id)
            .and_then(|a| a.app().developer.clone());
        kiwi.db.save_install(&InstallRow {
            device_id: attempt.device_id.clone(),
            signed_bundle_id: build.signed_bundle_id.clone(),
            source_id: build.source_id.clone(),
            bundle_id: build.bundle_id.clone(),
            name: build.app_name.clone(),
            icon_url: build.icon_url.clone(),
            developer,
            build_id: Some(build.id.clone()),
            version: build.version.clone(),
            build: build.build.clone(),
            release: Some(build.release.clone()),
            url_scheme: build.url_scheme.clone(),
            expires_at: build.expires_at,
            origin: "kiwi".into(),
            verified: false,
            installed_at: now,
            last_seen_at: None,
            removed_at: None,
        })?;
        tracing::info!(build = %build.id, device = %attempt.device_id, "app download completed; install recorded");
        observe_device(kiwi, &attempt.device_id, ua);
        Ok(())
    })();
    if let Err(e) = result {
        tracing::error!("could not record completed download: {e}");
    }
    kiwi.kick.notify_one();
}

pub async fn icon_handler(
    State(kiwi): State<Arc<Kiwi>>,
    UrlPath(token): UrlPath<String>,
) -> Response {
    let Some((_, build)) = lookup(&kiwi, &token) else {
        return not_found();
    };
    let Some(url) = build.icon_url else {
        return not_found();
    };
    match tokio::fs::read(icon_path(&kiwi.paths.icons, &url)).await {
        Ok(bytes) => {
            let jpeg = bytes.starts_with(&[0xff, 0xd8, 0xff]);
            let mut response = bytes.into_response();
            let headers = response.headers_mut();
            headers.insert(
                header::CONTENT_TYPE,
                HeaderValue::from_static(if jpeg { "image/jpeg" } else { "image/png" }),
            );
            headers.insert(
                header::CACHE_CONTROL,
                HeaderValue::from_static("public, max-age=86400"),
            );
            response
        }
        Err(_) => not_found(),
    }
}

/// Image bytes Kiwi caches as an icon (PNG or JPEG).
pub fn is_icon_image(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x89PNG\r\n\x1a\n") || bytes.starts_with(&[0xff, 0xd8, 0xff])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{db::BuildStatus, testutil::build_row};

    #[test]
    fn tokens_expire_unless_an_install_is_in_progress() {
        let now = Utc::now();
        let hours = |h| now - chrono::Duration::hours(h);
        let minutes = |m| now - chrono::Duration::minutes(m);
        let attempt = |created, used: Option<chrono::DateTime<Utc>>| AttemptRow {
            token: "t".into(),
            build_id: "b".into(),
            device_id: "d".into(),
            created_at: created,
            manifest_at: used,
            download_started_at: None,
            download_completed_at: None,
            user_agent: None,
        };
        // Fresh: works, kept.
        let a = attempt(hours(1), None);
        assert!(token_alive(&a, now) && !renew_token(&a, now));
        // Half its life: still works, but the next link is a new one.
        let a = attempt(hours(13), None);
        assert!(token_alive(&a, now) && renew_token(&a, now));
        // ... unless the installer is using it.
        let a = attempt(hours(13), Some(minutes(5)));
        assert!(token_alive(&a, now) && !renew_token(&a, now));
        // Past its lifetime: dead, unless an install is in progress, and never handed out.
        let a = attempt(hours(25), None);
        assert!(!token_alive(&a, now) && renew_token(&a, now));
        let a = attempt(hours(25), Some(minutes(5)));
        assert!(token_alive(&a, now) && renew_token(&a, now));
        let a = attempt(hours(49), Some(minutes(5)));
        assert!(!token_alive(&a, now));
    }

    #[test]
    fn retired_tokens_lead_to_their_replacement() {
        let retired = RetiredTokens::default();
        retired.add("old".into(), "new".into());
        assert_eq!(retired.current("old").as_deref(), Some("new"));
        assert_eq!(retired.current("new"), None);
    }

    #[test]
    fn install_url_encodes_the_manifest_url() {
        assert_eq!(
            install_url(
                "https://mac-studio.example.ts.net/",
                "AbC-_1234567890123456z"
            ),
            "itms-services://?action=download-manifest&url=https%3A%2F%2Fmac-studio.example.ts.net%2Fota%2FAbC-_1234567890123456z%2Fmanifest.plist"
        );
    }

    #[test]
    fn manifest_lists_assets_and_metadata() {
        let mut build = build_row("b", BuildStatus::Ready, Utc::now());
        build.app_name = "Tom & Jerry <3".into();
        let xml = manifest("https://kiwi.test", "tok", &build, true);
        let value: Value = plist::from_bytes(&xml).unwrap();
        let item = &value.as_dictionary().unwrap()["items"].as_array().unwrap()[0];
        let item = item.as_dictionary().unwrap();
        let assets: Vec<(&str, &str)> = item["assets"]
            .as_array()
            .unwrap()
            .iter()
            .map(|a| {
                let a = a.as_dictionary().unwrap();
                (
                    a["kind"].as_string().unwrap(),
                    a["url"].as_string().unwrap(),
                )
            })
            .collect();
        assert_eq!(
            assets,
            vec![
                ("software-package", "https://kiwi.test/ota/tok/app.ipa"),
                ("display-image", "https://kiwi.test/ota/tok/icon.png"),
                ("full-size-image", "https://kiwi.test/ota/tok/icon.png"),
            ]
        );
        let meta = item["metadata"].as_dictionary().unwrap();
        assert_eq!(
            meta["bundle-identifier"].as_string(),
            Some("com.example.app.A1B2C3D4E5")
        );
        assert_eq!(meta["bundle-version"].as_string(), Some("1.0"));
        assert_eq!(meta["kind"].as_string(), Some("software"));
        assert_eq!(meta["title"].as_string(), Some("Tom & Jerry <3"));
        assert!(String::from_utf8_lossy(&xml).contains("Tom &amp; Jerry &lt;3"));
        let without_icon: Value =
            plist::from_bytes(&manifest("https://kiwi.test", "tok", &build, false)).unwrap();
        let item = &without_icon.as_dictionary().unwrap()["items"]
            .as_array()
            .unwrap()[0];
        assert_eq!(
            item.as_dictionary().unwrap()["assets"]
                .as_array()
                .unwrap()
                .len(),
            1
        );
    }

    #[test]
    fn parses_ranges() {
        use ByteRange::*;
        let r = |h: &str| parse_range(Some(h), 1000);
        assert_eq!(parse_range(None, 1000), Full);
        assert_eq!(r("bytes=0-499"), Partial { start: 0, end: 499 });
        assert_eq!(
            r("bytes=500-"),
            Partial {
                start: 500,
                end: 999
            }
        );
        assert_eq!(
            r("bytes=900-5000"),
            Partial {
                start: 900,
                end: 999
            }
        );
        assert_eq!(
            r("bytes=-100"),
            Partial {
                start: 900,
                end: 999
            }
        );
        assert_eq!(r("bytes=-5000"), Partial { start: 0, end: 999 });
        assert_eq!(
            r("bytes=999-999"),
            Partial {
                start: 999,
                end: 999
            }
        );
        assert_eq!(r("bytes=1000-"), Unsatisfiable);
        assert_eq!(r("bytes=-0"), Unsatisfiable);
        assert_eq!(r("bytes=5-1"), Full);
        assert_eq!(r("bytes=0-1,5-6"), Full);
        assert_eq!(r("bytes=a-b"), Full);
        assert_eq!(r("items=0-1"), Full);
        assert_eq!(r("bytes=+1-2"), Full);
        assert_eq!(parse_range(Some("bytes=0-"), 0), Unsatisfiable);
    }

    #[test]
    fn served_ranges_complete_only_when_they_cover_the_file() {
        let mut ranges = Vec::new();
        merge_range(&mut ranges, 10, 19);
        merge_range(&mut ranges, 30, 39);
        merge_range(&mut ranges, 20, 29);
        assert_eq!(ranges, [(10, 39)]);
        merge_range(&mut ranges, 0, 5);
        merge_range(&mut ranges, 35, 50);
        assert_eq!(ranges, [(0, 5), (10, 50)]);

        let served = ServedRanges::default();
        // The zip tail alone (what reading the central directory fetches) is not an install.
        assert!(!served.add("tok", 1000, 900, 999));
        assert!(!served.add("tok", 1000, 0, 99));
        assert!(!served.add("other", 1000, 100, 899));
        assert!(served.add("tok", 1000, 100, 899));
        // Completed tokens start over.
        assert!(!served.add("tok", 1000, 900, 999));
        assert!(served.add("tok2", 1000, 0, 999));
        // A different file length (rebuilt IPA) resets the token's ranges.
        assert!(!served.add("tok3", 1000, 0, 499));
        assert!(!served.add("tok3", 2000, 500, 1999));
    }

    #[test]
    fn icon_paths_and_types() {
        let p = icon_path(Path::new("/icons"), "https://example.com/i.png");
        assert_eq!(p.extension().unwrap(), "png");
        assert_eq!(p.file_stem().unwrap().len(), 64);
        assert!(is_icon_image(b"\x89PNG\r\n\x1a\nrest"));
        assert!(!is_icon_image(b"<html>"));
    }

    #[tokio::test]
    async fn tracked_stream_completes_only_after_the_last_byte() {
        use futures_util::StreamExt;
        use std::sync::atomic::{AtomicUsize, Ordering};
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("f");
        std::fs::write(&path, vec![7u8; 1000]).unwrap();
        let fired = Arc::new(AtomicUsize::new(0));
        let stream = |count: u64| {
            let (fired, path) = (fired.clone(), path.clone());
            async move {
                let file = tokio::fs::File::open(&path).await.unwrap();
                TrackedStream {
                    inner: ReaderStream::with_capacity(file.take(count), 100),
                    remaining: count,
                    on_complete: Some(Box::new(move || {
                        fired.fetch_add(1, Ordering::SeqCst);
                    })),
                }
            }
        };
        // Dropped half way: not complete.
        let mut s = stream(1000).await;
        s.next().await.unwrap().unwrap();
        drop(s);
        assert_eq!(fired.load(Ordering::SeqCst), 0);
        // Fully read: complete exactly once.
        let s = stream(1000).await;
        let total: usize = s
            .map(|c| c.unwrap().len())
            .fold(0, |a, b| async move { a + b })
            .await;
        assert_eq!(total, 1000);
        assert_eq!(fired.load(Ordering::SeqCst), 1);
        // A file shorter than promised is an error, not a completion.
        let mut s = stream(2000).await;
        let mut last = None;
        while let Some(item) = s.next().await {
            last = Some(item);
        }
        assert!(last.unwrap().is_err());
        drop(s);
        assert_eq!(fired.load(Ordering::SeqCst), 1);
    }
}
