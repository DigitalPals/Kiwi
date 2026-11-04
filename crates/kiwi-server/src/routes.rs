//! HTTP routes: `/api` (JSON, CSRF-checked), `/ota` and the web app.

use std::{
    net::SocketAddr,
    path::{Component, Path as FsPath},
    sync::{Arc, Mutex},
    time::Instant,
};

use axum::{
    Json, Router,
    body::{Body, Bytes},
    extract::{ConnectInfo, FromRequest, FromRequestParts, Path, RawQuery, Request, State},
    http::{HeaderMap, HeaderValue, Method, StatusCode, Uri, header, request::Parts},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{delete, get, patch, post},
};
use chrono::{Duration, Utc};
use serde::{Deserialize, Deserializer, de::DeserializeOwned};
use serde_json::{Map, Value, json};

use crate::{
    Kiwi, builds,
    catalog::SourceJson,
    db::{BrowseMode, DeviceRow, OriginalChoice, SourceRow},
    devices::{self, COOKIE, Requester, device_json},
    error::{ApiError, ApiResult},
    ota, owner,
    push::{self, Payload, SubscribeBody},
    sources,
    store::{self, StoreAppJson},
    views::{self, AppDetail, AppSummary, View},
};

pub fn router(kiwi: Arc<Kiwi>) -> Router {
    let api = Router::new()
        .route("/health", get(health))
        .route("/automation", get(automation_overview))
        .route(
            "/automation/{app}",
            get(automation_state)
                .patch(automation_preferences)
                .delete(automation_remove),
        )
        .route("/automation/{app}/add", post(automation_add))
        .route("/automation/{app}/check", post(automation_check))
        .route("/automation/{app}/cancel", post(automation_cancel))
        .route("/me", get(me).patch(patch_me))
        .route("/sources", get(list_sources).post(add_source))
        .route("/sources/refresh", post(refresh_sources))
        .route("/sources/{id}", patch(patch_source).delete(delete_source))
        .route("/sources/{id}/refresh", post(refresh_source))
        .route("/icons/{file}", get(icon))
        .route("/apps", get(list_apps))
        .route(
            "/apps/{source_id}/{bundle_id}",
            get(app_detail).patch(patch_app),
        )
        .route("/apps/{source_id}/{bundle_id}/prepare", post(prepare))
        .route("/store/search", get(store_search))
        .route("/builds/{id}", get(get_build))
        .route("/library", get(library))
        .route("/library/{signed_bundle_id}", delete(forget))
        .route("/updates", get(updates))
        .route("/updates/prepare", post(prepare_updates))
        .route("/signing", get(signing))
        .route("/signing/rescan", post(rescan_signing))
        .route("/settings", get(get_settings).patch(patch_settings))
        .route("/push/subscribe", post(push_subscribe))
        .route("/push/unsubscribe", post(push_unsubscribe))
        .route("/push/test", post(push_test))
        .route("/devices", get(list_devices))
        .route("/devices/sync", post(sync_devices))
        .route("/devices/{id}", patch(patch_device))
        .fallback(|| async { ApiError::not_found("Unknown API endpoint.") })
        .method_not_allowed_fallback(|| async {
            ApiError::new(
                StatusCode::METHOD_NOT_ALLOWED,
                "bad_request",
                "Method not allowed.",
            )
        })
        .layer(middleware::from_fn(api_guard));
    Router::new()
        .nest("/api", api)
        .route("/ota/{token}/manifest.plist", get(ota::manifest_handler))
        .route("/ota/{token}/app.ipa", get(ota::ipa_handler))
        .route("/ota/{token}/icon.png", get(ota::icon_handler))
        .fallback(web)
        .layer(middleware::from_fn_with_state(kiwi.clone(), host_guard))
        .layer(middleware::from_fn(log_requests))
        .with_state(kiwi)
}

/// Rejects requests for other host names (DNS rebinding from a browser on the Mac could
/// otherwise reach the loopback port with a forged `X-Forwarded-For`).
async fn host_guard(State(kiwi): K, req: Request, next: Next) -> Response {
    let headers = req.headers();
    let host = headers
        .get(header::HOST)
        .and_then(|v| v.to_str().ok())
        .or_else(|| req.uri().authority().map(|a| a.as_str()));
    let forwarded = headers
        .get_all("x-forwarded-host")
        .iter()
        .collect::<Vec<_>>();
    let forwarded = match forwarded.as_slice() {
        [] => None,
        [one] => Some(one.to_str().unwrap_or("")),
        _ => Some(""),
    };
    if !kiwi.hosts.allows(host, forwarded) {
        tracing::warn!(
            "rejected a request for host {:?} (forwarded host {:?})",
            host.unwrap_or(""),
            forwarded.unwrap_or("")
        );
        return ApiError::bad_host().into_response();
    }
    next.run(req).await
}

async fn log_requests(req: Request, next: Next) -> Response {
    let method = req.method().clone();
    let path = match req.uri().path().strip_prefix("/ota/") {
        // Tokens authorize downloads; keep them out of the logs.
        Some(rest) => format!("/ota/…/{}", rest.split_once('/').map_or("", |(_, f)| f)),
        None => req.uri().path().to_owned(),
    };
    let started = Instant::now();
    let response = next.run(req).await;
    let status = response.status();
    let ms = started.elapsed().as_millis();
    if method != Method::GET || status.is_client_error() || status.is_server_error() {
        tracing::info!("{method} {path} {} {ms}ms", status.as_u16());
    } else {
        tracing::debug!("{method} {path} {} {ms}ms", status.as_u16());
    }
    response
}

// ---- guard, extractors ----

/// Receives a new `kiwi_device` cookie from [`Ctx`] for the response.
#[derive(Clone, Default)]
struct CookieSlot(Arc<Mutex<Option<String>>>);

fn check_csrf(headers: &HeaderMap) -> ApiResult<()> {
    if headers.get("x-kiwi").and_then(|v| v.to_str().ok()) != Some("1") {
        return Err(ApiError::csrf(
            "Missing X-Kiwi header; requests must come from the Kiwi app.",
        ));
    }
    let has_body = headers
        .get(header::CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok()?.parse::<u64>().ok())
        .is_some_and(|n| n > 0)
        || headers.contains_key(header::TRANSFER_ENCODING);
    let json = headers
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        .is_some_and(|v| v.trim().eq_ignore_ascii_case("application/json"));
    if has_body && !json {
        return Err(ApiError::csrf(
            "Requests with a body must use Content-Type: application/json.",
        ));
    }
    Ok(())
}

/// CSRF rule for mutations, `Cache-Control: no-store`, and the device cookie.
async fn api_guard(mut req: Request, next: Next) -> Response {
    let mutating = matches!(
        *req.method(),
        Method::POST | Method::PATCH | Method::DELETE | Method::PUT
    );
    let mut response = match mutating.then(|| check_csrf(req.headers())) {
        Some(Err(e)) => e.into_response(),
        _ => {
            let slot = CookieSlot::default();
            req.extensions_mut().insert(slot.clone());
            let mut response = next.run(req).await;
            if let Some(id) = slot.0.lock().unwrap().take() {
                let cookie =
                    format!("{COOKIE}={id}; Path=/; Max-Age=315360000; HttpOnly; SameSite=Lax");
                if let Ok(v) = HeaderValue::from_str(&cookie) {
                    response.headers_mut().append(header::SET_COOKIE, v);
                }
            }
            response
        }
    };
    response
        .headers_mut()
        .entry(header::CACHE_CONTROL)
        .or_insert(HeaderValue::from_static("no-store"));
    response
}

/// The requesting device and the public URL for links. Only the owner's devices get one:
/// other people's get `403 not_owner`, so every API handler but `/api/health` takes it.
pub struct Ctx {
    pub device: DeviceRow,
    pub public_url: String,
}

impl FromRequestParts<Arc<Kiwi>> for Ctx {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, kiwi: &Arc<Kiwi>) -> ApiResult<Self> {
        let text = |name: header::HeaderName| {
            let values: Vec<&str> = parts
                .headers
                .get_all(name)
                .iter()
                .filter_map(|v| v.to_str().ok())
                .collect();
            (!values.is_empty()).then(|| values.join("; "))
        };
        let requester = Requester {
            peer: parts
                .extensions
                .get::<ConnectInfo<SocketAddr>>()
                .map(|c| c.0),
            forwarded_for: parts
                .headers
                .get("x-forwarded-for")
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned),
            cookie: text(header::COOKIE),
            user_agent: parts
                .headers
                .get(header::USER_AGENT)
                .and_then(|v| v.to_str().ok())
                .map(str::to_owned),
        };
        let (device, cookie) = devices::resolve(kiwi, &requester).await?;
        if let Some(cookie) = cookie
            && let Some(slot) = parts.extensions.get::<CookieSlot>()
        {
            *slot.0.lock().unwrap() = Some(cookie);
        }
        owner::require(kiwi, &device)?;
        Ok(Self {
            device,
            public_url: kiwi.public_url(&parts.headers),
        })
    }
}

/// A JSON body; empty bodies read as `{}`.
pub struct JsonBody<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned> FromRequest<S> for JsonBody<T> {
    type Rejection = ApiError;
    async fn from_request(req: Request, _: &S) -> ApiResult<Self> {
        let bytes: Bytes = axum::body::to_bytes(req.into_body(), 64 * 1024)
            .await
            .map_err(|_| ApiError::bad_request("Request body is too large."))?;
        let bytes: &[u8] = if bytes.iter().all(u8::is_ascii_whitespace) {
            b"{}"
        } else {
            &bytes
        };
        serde_json::from_slice(bytes)
            .map(JsonBody)
            .map_err(|e| ApiError::bad_request(format!("Invalid request body: {e}")))
    }
}

/// Path parameters with JSON errors.
pub struct P<T>(pub T);

impl<S: Send + Sync, T: DeserializeOwned + Send> FromRequestParts<S> for P<T> {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, state: &S) -> ApiResult<Self> {
        Path::<T>::from_request_parts(parts, state)
            .await
            .map(|p| P(p.0))
            .map_err(|e| ApiError::bad_request(e.body_text()))
    }
}

/// Distinguishes a missing field from `null`.
fn nullable<'de, D: Deserializer<'de>, T: Deserialize<'de>>(
    d: D,
) -> Result<Option<Option<T>>, D::Error> {
    Option::<T>::deserialize(d).map(Some)
}

type K = State<Arc<Kiwi>>;

// ---- me, devices ----

/// `pid` lets the deploy script check that the process launchd started is the one answering.
async fn health() -> Json<Value> {
    Json(json!({
        "ok": true,
        "version": env!("CARGO_PKG_VERSION"),
        "pid": std::process::id(),
    }))
}

fn warning(level: &str, code: &str, message: String, link: &str) -> Value {
    json!({ "level": level, "code": code, "message": message, "link": link })
}

fn days_until(t: chrono::DateTime<Utc>, now: chrono::DateTime<Utc>) -> String {
    match (t - now).num_days().max(0) {
        0 => "today".into(),
        1 => "in 1 day".into(),
        n => format!("in {n} days"),
    }
}

async fn me_json(kiwi: &Kiwi, ctx: &Ctx) -> ApiResult<Value> {
    let view = View::load(kiwi, &ctx.device, &ctx.public_url)?;
    let now = view.now;
    let sources = kiwi.catalog.sources();
    let errors = sources
        .iter()
        .filter(|s| s.enabled && s.source.error.is_some())
        .count();
    let mut warnings = Vec::new();
    if view.signing.identities.is_empty() {
        let message = view.signing.error(now).unwrap_or_default();
        warnings.push(warning("error", "no_identity", message, "/settings"));
    } else if let Some(message) = view.signing.error(now) {
        warnings.push(warning("error", "no_profile", message, "/settings"));
    } else if let Some(active) = view.signing.active(now)
        && active.expires_at - now < Duration::days(30)
    {
        let message = format!(
            "The signing certificate or profile expires {}. Renew it in Xcode, then rescan.",
            days_until(active.expires_at, now)
        );
        warnings.push(warning("warning", "expiring", message, "/settings"));
    }
    if errors > 0 {
        let what = if errors == 1 {
            "1 source".into()
        } else {
            format!("{errors} sources")
        };
        warnings.push(warning(
            "warning",
            "source_errors",
            format!("{what} failed to refresh. Check Sources for details."),
            "/sources",
        ));
    }
    for install in &view.installs {
        if let Some(expires) = install.expires_at.filter(|_| install.origin == "kiwi")
            && expires - now < Duration::days(7)
        {
            let message = if expires <= now {
                format!("{} has expired. Update it to re-sign.", install.name)
            } else {
                format!(
                    "{} expires {}. Update it to re-sign.",
                    install.name,
                    days_until(expires, now)
                )
            };
            warnings.push(warning("warning", "expiring", message, "/apps"));
        }
    }
    if ctx.device.os_version.is_none() {
        warnings.push(warning(
            "info",
            "unknown_os",
            "Kiwi doesn't know this device's iOS version yet, so every release is offered. Set it in Settings.".into(),
            "/settings",
        ));
    }
    let preparing = kiwi
        .db
        .builds()?
        .iter()
        .filter(|b| b.status.is_active())
        .count();
    Ok(json!({
        "device": device_json(&ctx.device),
        "server": {
            "version": env!("CARGO_PKG_VERSION"),
            "public_url": ctx.public_url,
            "team_id": view.team,
            "devicectl_available": kiwi.devicectl_available.load(std::sync::atomic::Ordering::Relaxed),
        },
        "push": {
            "vapid_public_key": kiwi.vapid_public_key,
            "subscribed": !kiwi.db.subscriptions(&ctx.device.id)?.is_empty(),
        },
        "counts": {
            "updates": view.updates().len(),
            "installed": view.installs.len(),
            "sources": sources.len(),
            "source_errors": errors,
            "preparing": preparing,
        },
        "warnings": warnings,
    }))
}

async fn me(State(kiwi): K, ctx: Ctx) -> ApiResult<Json<Value>> {
    me_json(&kiwi, &ctx).await.map(Json)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct MePatch {
    name: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    os_version: Option<Option<String>>,
}

fn device_name(name: &str) -> ApiResult<Option<String>> {
    let name = name.trim();
    if name.chars().count() > 64 {
        return Err(ApiError::bad_request(
            "Names can have at most 64 characters.",
        ));
    }
    Ok((!name.is_empty()).then(|| name.to_owned()))
}

async fn patch_me(
    State(kiwi): K,
    mut ctx: Ctx,
    JsonBody(body): JsonBody<MePatch>,
) -> ApiResult<Json<Value>> {
    let id = ctx.device.id.clone();
    let name = body.name.as_deref().map(device_name).transpose()?;
    let os_version = match body.os_version {
        Some(Some(v)) => {
            let v = v.trim();
            let valid = v.split('.').count() <= 4
                && v.split('.').all(|p| (1..=4).contains(&p.len()))
                && kiwi_source::compare_numeric(v, "0").is_some();
            if !valid {
                return Err(ApiError::bad_request("Enter an iOS version like 26.1."));
            }
            Some(Some(v.to_owned()))
        }
        other => other,
    };
    if let Some(name) = name {
        kiwi.db.set_device_name(&id, name.as_deref())?;
    }
    if let Some(version) = os_version {
        kiwi.db.set_manual_os(&id, version.as_deref())?;
    }
    if let Some(device) = kiwi.db.device(&id)? {
        ctx.device = device;
    }
    me_json(&kiwi, &ctx).await.map(Json)
}

async fn list_devices(State(kiwi): K, _: Ctx) -> ApiResult<Json<devices::DevicesJson>> {
    devices::devices_json(&kiwi, false).await.map(Json)
}

async fn sync_devices(State(kiwi): K, _: Ctx) -> ApiResult<Json<devices::DevicesJson>> {
    devices::sync(&kiwi).await?;
    devices::devices_json(&kiwi, false).await.map(Json)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct DevicePatch {
    name: Option<String>,
    #[serde(default, deserialize_with = "nullable")]
    devicectl_id: Option<Option<String>>,
}

async fn patch_device(
    State(kiwi): K,
    _: Ctx,
    P(id): P<String>,
    JsonBody(body): JsonBody<DevicePatch>,
) -> ApiResult<Json<devices::DeviceJson>> {
    let not_found = || ApiError::not_found("Device not found.");
    kiwi.db.device(&id)?.ok_or_else(not_found)?;
    let name = body.name.as_deref().map(device_name).transpose()?;
    if let Some(core) = body.devicectl_id
        && let Some(core) = devices::link(&kiwi, &id, core.as_deref()).await?
        && let Some(v) = &core.os_version
        && core.reachable()
    {
        kiwi.db.observe_device_os(&id, v, "devicectl")?;
    }
    if let Some(name) = name {
        kiwi.db.set_device_name(&id, name.as_deref())?;
    }
    let device = kiwi.db.device(&id)?.ok_or_else(not_found)?;
    Ok(Json(device_json(&device)))
}

// ---- sources ----

fn source_json(kiwi: &Kiwi, row: &SourceRow) -> ApiResult<Json<SourceJson>> {
    Ok(Json(SourceJson::new(row, &kiwi.db.app_prefs()?)))
}

async fn list_sources(State(kiwi): K, _: Ctx) -> ApiResult<Json<Vec<SourceJson>>> {
    sources_json(&kiwi)
}

fn sources_json(kiwi: &Kiwi) -> ApiResult<Json<Vec<SourceJson>>> {
    let prefs = kiwi.db.app_prefs()?;
    Ok(Json(
        kiwi.catalog
            .sources()
            .iter()
            .map(|s| SourceJson::new(s, &prefs))
            .collect(),
    ))
}

#[derive(Deserialize)]
struct AddSource {
    url: String,
}

async fn add_source(
    State(kiwi): K,
    _: Ctx,
    JsonBody(body): JsonBody<AddSource>,
) -> ApiResult<Response> {
    // Adding an IPA downloads it, which can outlast the request (the iPhone may suspend the
    // web app): the source is added even when nobody waits for the answer.
    let task = tokio::spawn({
        let kiwi = kiwi.clone();
        async move { sources::add(&kiwi, &body.url).await }
    });
    let row = task.await.map_err(ApiError::internal)??;
    Ok((StatusCode::CREATED, source_json(&kiwi, &row)?).into_response())
}

/// An icon Kiwi took from an IPA (see [`crate::linked`]). Its name is its SHA-256.
async fn icon(State(kiwi): K, _: Ctx, P(file): P<String>) -> ApiResult<Response> {
    let not_found = || ApiError::not_found("Icon not found.");
    let path = crate::linked::icon_file(&kiwi, &file).ok_or_else(not_found)?;
    let bytes = tokio::fs::read(path).await.map_err(|_| not_found())?;
    Ok((
        [
            (header::CONTENT_TYPE, "image/png"),
            (
                header::CACHE_CONTROL,
                "private, max-age=31536000, immutable",
            ),
        ],
        bytes,
    )
        .into_response())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SourcePatch {
    enabled: Option<bool>,
    browse: Option<BrowseMode>,
}

async fn patch_source(
    State(kiwi): K,
    _: Ctx,
    P(id): P<String>,
    JsonBody(body): JsonBody<SourcePatch>,
) -> ApiResult<Json<SourceJson>> {
    kiwi.catalog
        .source(&id)
        .ok_or_else(|| ApiError::not_found("Source not found."))?;
    if let Some(enabled) = body.enabled {
        sources::set_enabled(&kiwi, &id, enabled)?;
    }
    if let Some(browse) = body.browse {
        sources::set_browse(&kiwi, &id, browse)?;
    }
    let row = kiwi
        .catalog
        .source(&id)
        .ok_or_else(|| ApiError::not_found("Source not found."))?;
    source_json(&kiwi, &row)
}

async fn delete_source(State(kiwi): K, _: Ctx, P(id): P<String>) -> ApiResult<StatusCode> {
    sources::remove(&kiwi, &id)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn refresh_source(State(kiwi): K, _: Ctx, P(id): P<String>) -> ApiResult<Json<SourceJson>> {
    // Finishes without a waiting client too: an app added by link may download its IPA.
    let task = tokio::spawn({
        let kiwi = kiwi.clone();
        async move { sources::refresh(&kiwi, &id).await }
    });
    let row = task.await.map_err(ApiError::internal)??;
    kiwi.kick.notify_one();
    source_json(&kiwi, &row)
}

async fn refresh_sources(State(kiwi): K, _: Ctx) -> ApiResult<Json<Vec<SourceJson>>> {
    let ids = kiwi
        .catalog
        .sources()
        .iter()
        .filter(|s| s.enabled)
        .map(|s| s.id.clone())
        .collect();
    sources::refresh_many(&kiwi, ids).await;
    sources_json(&kiwi)
}

// ---- apps, builds ----

async fn list_apps(
    State(kiwi): K,
    ctx: Ctx,
    RawQuery(query): RawQuery,
) -> ApiResult<Json<Vec<AppSummary>>> {
    let mut q = None;
    let mut source = None;
    let mut hidden = false;
    for (k, v) in url::form_urlencoded::parse(query.unwrap_or_default().as_bytes()) {
        match k.as_ref() {
            "q" => q = Some(v.into_owned()),
            "source" => source = Some(v.into_owned()).filter(|s| !s.is_empty()),
            "hidden" => hidden = matches!(v.as_ref(), "1" | "true"),
            _ => {}
        }
    }
    let view = View::load(&kiwi, &ctx.device, &ctx.public_url)?;
    kiwi.catalog
        .apps(source.as_deref(), q.as_deref())
        .iter()
        .filter(|app| hidden || !view.hidden(app))
        .map(|app| view.summary(app, None))
        .collect::<ApiResult<_>>()
        .map(Json)
}

async fn app_detail(
    State(kiwi): K,
    ctx: Ctx,
    P((source_id, bundle_id)): P<(String, String)>,
) -> ApiResult<Response> {
    let app = kiwi
        .catalog
        .app(&source_id, &bundle_id)
        .ok_or_else(|| ApiError::not_found("App not found."))?;
    let view = View::load(&kiwi, &ctx.device, &ctx.public_url)?;
    Ok(Json(view.detail(&app)?).into_response())
}

/// `{ "bundle_id": null }`: not based on an App Store app.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct OriginalBody {
    bundle_id: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AppPatch {
    hidden: Option<bool>,
    /// `null`: the App Store app with the same bundle id again.
    #[serde(default, deserialize_with = "nullable")]
    original: Option<Option<OriginalBody>>,
}

/// Hides or shows an app in Browse and chooses the App Store app it is based on.
async fn patch_app(
    State(kiwi): K,
    ctx: Ctx,
    P((source_id, bundle_id)): P<(String, String)>,
    JsonBody(body): JsonBody<AppPatch>,
) -> ApiResult<Json<AppDetail>> {
    let app = kiwi
        .catalog
        .app(&source_id, &bundle_id)
        .ok_or_else(|| ApiError::not_found("App not found."))?;
    let now = Utc::now();
    let original = match body.original {
        None => None,
        Some(None) => Some(OriginalChoice::Auto),
        Some(Some(OriginalBody { bundle_id: None })) => Some(OriginalChoice::None),
        Some(Some(OriginalBody {
            bundle_id: Some(store_id),
        })) => {
            let store_id = store_id.trim().to_owned();
            if store_id.is_empty() || store_id.len() > 255 {
                return Err(ApiError::bad_request("Choose an App Store app."));
            }
            Some(OriginalChoice::App(
                known_store_app(&kiwi, &store_id).await?,
            ))
        }
    };
    if let Some(hidden) = body.hidden {
        kiwi.db
            .set_app_shown(&source_id, &bundle_id, Some(!hidden), now)?;
    }
    if let Some(original) = original {
        kiwi.db
            .set_app_original(&source_id, &bundle_id, &original, now)?;
        kiwi.kick.notify_one();
    }
    let view = View::load(&kiwi, &ctx.device, &ctx.public_url)?;
    Ok(Json(view.detail(&app)?))
}

/// The bundle id of an App Store app in the current storefront, looked up unless cached.
async fn known_store_app(kiwi: &Kiwi, store_id: &str) -> ApiResult<String> {
    let country = kiwi.settings().app_store_country;
    let cached = kiwi
        .db
        .store_rows(&country)?
        .into_iter()
        .find(|r| r.bundle_id.eq_ignore_ascii_case(store_id) && r.app.is_some());
    if let Some(row) = cached {
        return Ok(row.bundle_id);
    }
    let wanted = [store_id.to_owned()];
    let (found, missing) = store::lookup(kiwi, &country, &wanted)
        .await
        .map_err(store::unavailable)?;
    kiwi.db
        .save_store_lookup(&country, &found, &missing, Utc::now())?;
    match found.into_iter().next() {
        Some(app) => Ok(app.bundle_id),
        None => Err(ApiError::bad_request(format!(
            "No app in the App Store ({}) has the bundle ID {store_id}.",
            country.to_uppercase()
        ))),
    }
}

async fn store_search(
    State(kiwi): K,
    _: Ctx,
    RawQuery(query): RawQuery,
) -> ApiResult<Json<Vec<StoreAppJson>>> {
    let term = url::form_urlencoded::parse(query.unwrap_or_default().as_bytes())
        .find(|(k, _)| k == "term")
        .map(|(_, v)| v.trim().to_owned())
        .unwrap_or_default();
    if term.is_empty() || term.chars().count() > 100 {
        return Err(ApiError::bad_request("Enter an app name to search for."));
    }
    let country = kiwi.settings().app_store_country;
    let apps = store::search(&kiwi, &country, &term).await?;
    Ok(Json(apps.iter().map(StoreAppJson::from).collect()))
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PrepareBody {
    version: Option<String>,
    build: Option<String>,
    revision: Option<u64>,
}

async fn prepare(
    State(kiwi): K,
    ctx: Ctx,
    P((source_id, bundle_id)): P<(String, String)>,
    JsonBody(body): JsonBody<PrepareBody>,
) -> ApiResult<Response> {
    if (body.build.is_some() || body.revision.is_some()) && body.version.is_none() {
        return Err(ApiError::bad_request("A build needs a version."));
    }
    let build = builds::prepare(
        &kiwi,
        &source_id,
        &bundle_id,
        body.version.as_deref(),
        body.build.as_deref(),
        body.revision,
        &ctx.device,
    )
    .await?;
    let build = views::build_json(&kiwi, &ctx.device.id, &ctx.public_url, &build, Utc::now())?;
    Ok((StatusCode::ACCEPTED, Json(build)).into_response())
}

async fn get_build(State(kiwi): K, ctx: Ctx, P(id): P<String>) -> ApiResult<Response> {
    let build = kiwi
        .db
        .build(&id)?
        .ok_or_else(|| ApiError::not_found("Build not found."))?;
    Ok(Json(views::build_json(
        &kiwi,
        &ctx.device.id,
        &ctx.public_url,
        &build,
        Utc::now(),
    )?)
    .into_response())
}

async fn library(State(kiwi): K, ctx: Ctx) -> ApiResult<Response> {
    let view = View::load(&kiwi, &ctx.device, &ctx.public_url)?;
    Ok(Json(view.library()?).into_response())
}

/// Forgets the install record (idempotent); the app stays on the phone.
async fn forget(State(kiwi): K, ctx: Ctx, P(signed): P<String>) -> ApiResult<StatusCode> {
    kiwi.db.delete_install(&ctx.device.id, &signed)?;
    Ok(StatusCode::NO_CONTENT)
}

async fn updates(State(kiwi): K, ctx: Ctx) -> ApiResult<Response> {
    let view = View::load(&kiwi, &ctx.device, &ctx.public_url)?;
    let list = view
        .updates()
        .iter()
        .map(|(app, _)| view.summary(app, None))
        .collect::<ApiResult<Vec<_>>>()?;
    Ok(Json(list).into_response())
}

async fn prepare_updates(State(kiwi): K, ctx: Ctx) -> ApiResult<Response> {
    let targets: Vec<_> = {
        let view = View::load(&kiwi, &ctx.device, &ctx.public_url)?;
        view.updates()
            .into_iter()
            .filter_map(|(app, _)| {
                let target = view.decide(&app, view.installed_for(&app)).target?;
                Some((app, target))
            })
            .collect()
    };
    let mut prepared = Vec::new();
    for (app, target) in targets {
        prepared.push(builds::prepare_release(&kiwi, &app, target).await?);
    }
    let now = Utc::now();
    let list = prepared
        .iter()
        .map(|b| views::build_json(&kiwi, &ctx.device.id, &ctx.public_url, b, now))
        .collect::<ApiResult<Vec<_>>>()?;
    Ok(Json(list).into_response())
}

// ---- signing, settings, push ----

async fn signing(State(kiwi): K, _: Ctx) -> Response {
    Json(kiwi.signing_info().json(Utc::now())).into_response()
}

async fn rescan_signing(State(kiwi): K, _: Ctx) -> Response {
    Json(kiwi.rescan_signing().await.json(Utc::now())).into_response()
}

async fn get_settings(State(kiwi): K, _: Ctx) -> Response {
    Json(kiwi.settings()).into_response()
}

async fn automation_overview(State(kiwi): K, _: Ctx) -> ApiResult<Response> {
    crate::automation::require_on(&kiwi)?;
    Ok(Json(crate::automation::overview(&kiwi)).into_response())
}

async fn automation_state(State(kiwi): K, _: Ctx, P(app): P<String>) -> ApiResult<Response> {
    crate::automation::require_on(&kiwi)?;
    Ok(Json(crate::automation::app_view(&kiwi, &app)?).into_response())
}

async fn automation_preferences(
    State(kiwi): K,
    _: Ctx,
    P(app): P<String>,
    JsonBody(body): JsonBody<crate::automation::Preferences>,
) -> ApiResult<Response> {
    crate::automation::require_on(&kiwi)?;
    kiwi.automation.preferences(&kiwi, &app, body)?;
    Ok(Json(crate::automation::app_view(&kiwi, &app)?).into_response())
}

async fn automation_add(
    State(kiwi): K,
    _: Ctx,
    P(app): P<String>,
    JsonBody(body): JsonBody<crate::automation::Preferences>,
) -> ApiResult<Response> {
    crate::automation::require_on(&kiwi)?;
    crate::automation::add(&kiwi, &app, body)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(crate::automation::app_view(&kiwi, &app)?),
    )
        .into_response())
}

async fn automation_remove(State(kiwi): K, _: Ctx, P(app): P<String>) -> ApiResult<Response> {
    crate::automation::require_on(&kiwi)?;
    crate::automation::remove(&kiwi, &app)?;
    Ok(Json(crate::automation::app_view(&kiwi, &app)?).into_response())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct AutomationCheck {
    #[serde(default)]
    force: bool,
}

async fn automation_check(
    State(kiwi): K,
    _: Ctx,
    P(app): P<String>,
    JsonBody(body): JsonBody<AutomationCheck>,
) -> ApiResult<Response> {
    crate::automation::require_on(&kiwi)?;
    crate::automation::queue(&kiwi, &app, body.force)?;
    Ok((
        StatusCode::ACCEPTED,
        Json(crate::automation::app_view(&kiwi, &app)?),
    )
        .into_response())
}

async fn automation_cancel(State(kiwi): K, _: Ctx, P(app): P<String>) -> ApiResult<Response> {
    crate::automation::require_on(&kiwi)?;
    crate::automation::recipe(&app)?;
    crate::automation::cancel(&kiwi, &app);
    Ok(Json(crate::automation::app_view(&kiwi, &app)?).into_response())
}

async fn patch_settings(
    State(kiwi): K,
    _: Ctx,
    JsonBody(patch): JsonBody<Map<String, Value>>,
) -> ApiResult<Response> {
    let previous = kiwi.settings();
    let next = previous.patched(&patch)?;
    kiwi.db.save_settings(&next.pairs())?;
    kiwi.set_settings(next.clone());
    if previous.kiwi_builds && !next.kiwi_builds {
        crate::automation::cancel_all(&kiwi);
    }
    kiwi.kick.notify_one();
    Ok(Json(next).into_response())
}

async fn push_subscribe(
    State(kiwi): K,
    ctx: Ctx,
    JsonBody(body): JsonBody<SubscribeBody>,
) -> ApiResult<StatusCode> {
    push::subscribe(&kiwi, &ctx.device.id, body)?;
    kiwi.kick.notify_one();
    Ok(StatusCode::NO_CONTENT)
}

#[derive(Deserialize)]
struct Unsubscribe {
    endpoint: String,
}

async fn push_unsubscribe(
    State(kiwi): K,
    ctx: Ctx,
    JsonBody(body): JsonBody<Unsubscribe>,
) -> ApiResult<StatusCode> {
    kiwi.db
        .delete_subscription(&body.endpoint, Some(&ctx.device.id))?;
    Ok(StatusCode::NO_CONTENT)
}

async fn push_test(State(kiwi): K, ctx: Ctx) -> ApiResult<Json<Value>> {
    let badge = View::load(&kiwi, &ctx.device, &ctx.public_url)?
        .updates()
        .len();
    let payload = Payload {
        title: "Kiwi".into(),
        body: "Notifications are working.".into(),
        url: "/settings".into(),
        badge,
        tag: "test".into(),
    };
    let sent = push::send_to_device(&kiwi, &ctx.device.id, &payload).await?;
    Ok(Json(json!({ "sent": sent })))
}

// ---- web app ----

/// Requests for missing files with these extensions get a 404 instead of the app shell.
#[rustfmt::skip]
const ASSET_EXTENSIONS: &[&str] = &[
    "js", "mjs", "css", "map", "png", "jpg", "jpeg", "gif", "svg", "ico", "webp", "json",
    "webmanifest", "txt", "woff", "woff2", "ttf", "html", "xml", "wasm", "ipa", "plist",
];

fn content_type(path: &FsPath) -> &'static str {
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "css" => "text/css; charset=utf-8",
        "json" | "map" => "application/json",
        "webmanifest" => "application/manifest+json",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "webp" => "image/webp",
        "txt" => "text/plain; charset=utf-8",
        "woff" => "font/woff",
        "woff2" => "font/woff2",
        "ttf" => "font/ttf",
        "xml" => "application/xml",
        "wasm" => "application/wasm",
        _ => "application/octet-stream",
    }
}

/// Static files from `--web-dir`; unknown paths without an asset extension serve
/// `index.html` (client-side routes such as `/app/<source>/<bundle.id>`).
async fn web(State(kiwi): K, method: Method, uri: Uri) -> Response {
    let path = uri.path();
    if !matches!(method, Method::GET | Method::HEAD) {
        return (StatusCode::METHOD_NOT_ALLOWED, "Method not allowed").into_response();
    }
    if path == "/ota" || path.starts_with("/ota/") {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    }
    let relative = path.trim_start_matches('/');
    let safe = !relative.contains('%')
        && !relative.contains('\\')
        && FsPath::new(relative)
            .components()
            .all(|c| matches!(c, Component::Normal(s) if !s.to_string_lossy().starts_with('.')));
    let root = &kiwi.config.web_dir;
    let file = if relative.is_empty() {
        root.join("index.html")
    } else {
        root.join(relative)
    };
    if safe && tokio::fs::metadata(&file).await.is_ok_and(|m| m.is_file()) {
        return serve_file(&file, relative).await;
    }
    let asset = FsPath::new(relative)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| ASSET_EXTENSIONS.contains(&e.to_ascii_lowercase().as_str()));
    if asset || relative.starts_with("assets/") {
        return (StatusCode::NOT_FOUND, "Not found").into_response();
    }
    serve_file(&root.join("index.html"), "index.html").await
}

async fn serve_file(file: &FsPath, relative: &str) -> Response {
    let Ok(bytes) = tokio::fs::read(file).await else {
        return (
            StatusCode::NOT_FOUND,
            "The Kiwi web app is missing: build web/ and pass --web-dir.",
        )
            .into_response();
    };
    let cache = if relative.starts_with("assets/") {
        "public, max-age=31536000, immutable"
    } else {
        "no-cache"
    };
    (
        [
            (header::CONTENT_TYPE, content_type(file)),
            (header::CACHE_CONTROL, cache),
        ],
        Body::from(bytes),
    )
        .into_response()
}
