//! The App Store apps that patched apps are based on (architecture.md "Original apps"): lookups by
//! bundle id and searches through Apple's iTunes Search API, cached in `store_apps` per
//! storefront.

use std::{
    collections::{HashMap, HashSet},
    sync::Mutex,
    time::{Duration, Instant},
};

use chrono::{DateTime, Utc};
use kiwi_source::Url;
use serde::{Deserialize, Serialize};

use crate::{
    Kiwi,
    db::{OriginalChoice, StoreApp, StoreRow, Time},
    error::ApiError,
};

pub const DEFAULT_URL: &str = "https://itunes.apple.com";
/// Bundle ids per lookup request.
const BATCH: usize = 20;
const MAX_RESPONSE: u64 = 4 * 1024 * 1024;
/// Apps found are looked up again after this long (for their current version).
pub const FOUND_TTL: chrono::Duration = chrono::Duration::hours(12);
/// Bundle ids without an App Store app are looked up again after this long.
pub const MISSING_TTL: chrono::Duration = chrono::Duration::days(7);
/// After a failed lookup, the scheduler waits this long before trying again.
const RETRY_AFTER: Duration = Duration::from_secs(15 * 60);
const SEARCH_LIMIT: usize = 15;

/// The iTunes Search API endpoint and the scheduler's retry state.
pub struct Store {
    base: Url,
    failed_at: Mutex<Option<Instant>>,
}

impl Store {
    pub fn new(base: &str) -> Result<Self, String> {
        let base = Url::parse(base).map_err(|e| format!("App Store URL {base}: {e}"))?;
        Ok(Self {
            base,
            failed_at: Mutex::default(),
        })
    }

    fn url(&self, path: &str, query: &[(&str, &str)]) -> Url {
        let mut url = self.base.clone();
        url.set_path(&format!(
            "{}/{path}",
            self.base.path().trim_end_matches('/')
        ));
        url.query_pairs_mut().extend_pairs(query);
        url
    }
}

/// `StoreApp` in api.md.
#[derive(Debug, Clone, Serialize)]
pub struct StoreAppJson {
    pub bundle_id: String,
    pub name: String,
    pub short_name: String,
    pub developer: Option<String>,
    pub version: String,
    pub released_at: Option<Time>,
    pub icon_url: Option<String>,
    pub store_url: String,
}

impl From<&StoreApp> for StoreAppJson {
    fn from(a: &StoreApp) -> Self {
        Self {
            bundle_id: a.bundle_id.clone(),
            name: a.name.clone(),
            short_name: short_name(&a.name),
            developer: a.developer.clone(),
            version: a.version.clone(),
            released_at: a.released_at,
            icon_url: a.icon_url.clone(),
            store_url: a.store_url.clone(),
        }
    }
}

/// The name without the tagline App Store names often carry: "Spotify: Music and Podcasts"
/// is "Spotify".
pub fn short_name(name: &str) -> String {
    let name = name.trim();
    [": ", " - ", " – ", " — ", " | "]
        .iter()
        .filter_map(|sep| name.find(sep))
        .min()
        .map(|i| name[..i].trim())
        .filter(|short| short.chars().count() >= 2)
        .unwrap_or(name)
        .to_owned()
}

#[derive(Deserialize)]
struct Results {
    #[serde(default)]
    results: Vec<Item>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct Item {
    kind: Option<String>,
    bundle_id: Option<String>,
    track_id: Option<i64>,
    track_name: Option<String>,
    artist_name: Option<String>,
    seller_name: Option<String>,
    version: Option<String>,
    current_version_release_date: Option<String>,
    artwork_url512: Option<String>,
    artwork_url100: Option<String>,
    track_view_url: Option<String>,
}

/// iOS apps in a lookup or search response; other kinds and incomplete entries are skipped.
pub fn parse_results(bytes: &[u8]) -> Result<Vec<StoreApp>, String> {
    let results: Results =
        serde_json::from_slice(bytes).map_err(|e| format!("Invalid App Store response: {e}"))?;
    Ok(results
        .results
        .into_iter()
        .filter(|i| i.kind.as_deref().is_none_or(|k| k == "software"))
        .filter_map(|i| {
            let mut store_url = Url::parse(i.track_view_url.as_deref()?).ok()?;
            // `?uo=4` is a tracking parameter.
            store_url.set_query(None);
            Some(StoreApp {
                bundle_id: i.bundle_id.filter(|b| !b.is_empty())?,
                track_id: i.track_id?,
                name: i.track_name.filter(|n| !n.trim().is_empty())?,
                developer: i.artist_name.or(i.seller_name),
                version: i.version.filter(|v| !v.trim().is_empty())?,
                released_at: i
                    .current_version_release_date
                    .and_then(|d| DateTime::parse_from_rfc3339(&d).ok())
                    .map(|d| d.with_timezone(&Utc)),
                icon_url: i.artwork_url512.or(i.artwork_url100),
                store_url: store_url.to_string(),
            })
        })
        .collect())
}

async fn fetch(kiwi: &Kiwi, url: &Url) -> Result<Vec<StoreApp>, String> {
    let bytes = kiwi
        .sources
        .json_bytes("App Store", url, MAX_RESPONSE, &kiwi.cancel)
        .await
        .map_err(|e| e.to_string())?;
    parse_results(&bytes)
}

/// Looks bundle ids up in a storefront. Returns the apps found under the requested ids (the
/// lookup ignores case) and the ids without an App Store app.
pub async fn lookup(
    kiwi: &Kiwi,
    country: &str,
    bundle_ids: &[String],
) -> Result<(Vec<StoreApp>, Vec<String>), String> {
    let mut found = Vec::new();
    let mut missing = Vec::new();
    for chunk in bundle_ids.chunks(BATCH) {
        let ids = chunk.join(",");
        let url = kiwi.store.url(
            "lookup",
            &[
                ("bundleId", &ids),
                ("country", country),
                ("entity", "software"),
            ],
        );
        let mut apps: HashMap<String, StoreApp> = fetch(kiwi, &url)
            .await?
            .into_iter()
            .map(|a| (a.bundle_id.to_lowercase(), a))
            .collect();
        for id in chunk {
            match apps.remove(&id.to_lowercase()) {
                Some(app) => found.push(StoreApp {
                    bundle_id: id.clone(),
                    ..app
                }),
                None => missing.push(id.clone()),
            }
        }
    }
    Ok((found, missing))
}

/// Searches a storefront's iOS apps by name; the results are cached so one can be linked.
pub async fn search(kiwi: &Kiwi, country: &str, term: &str) -> Result<Vec<StoreApp>, ApiError> {
    let limit = SEARCH_LIMIT.to_string();
    let url = kiwi.store.url(
        "search",
        &[
            ("term", term),
            ("country", country),
            ("entity", "software"),
            ("limit", &limit),
        ],
    );
    let apps = fetch(kiwi, &url).await.map_err(unavailable)?;
    kiwi.db.save_store_lookup(country, &apps, &[], Utc::now())?;
    Ok(apps)
}

pub fn unavailable(message: String) -> ApiError {
    ApiError::new(
        axum::http::StatusCode::BAD_GATEWAY,
        "app_store",
        format!("Couldn't reach the App Store. {message}"),
    )
}

/// Whether a cached lookup should be repeated.
pub fn stale(row: &StoreRow, now: Time) -> bool {
    let ttl = if row.app.is_some() {
        FOUND_TTL
    } else {
        MISSING_TTL
    };
    now - row.checked_at >= ttl
}

/// The App Store bundle id an app's original is looked up by, if any.
pub fn wanted_id<'a>(bundle_id: &'a str, choice: &'a OriginalChoice) -> Option<&'a str> {
    match choice {
        OriginalChoice::Auto => Some(bundle_id),
        OriginalChoice::None => None,
        OriginalChoice::App(b) => Some(b),
    }
}

/// Looks up the App Store apps of enabled sources' apps that are not cached for the current
/// storefront or are stale. Waits [`RETRY_AFTER`] after a failure. Returns how many bundle
/// ids were looked up.
pub async fn refresh(kiwi: &Kiwi) -> Result<usize, String> {
    if kiwi
        .store
        .failed_at
        .lock()
        .unwrap()
        .is_some_and(|t| t.elapsed() < RETRY_AFTER)
    {
        return Ok(0);
    }
    let country = kiwi.settings().app_store_country;
    let now = Utc::now();
    let prefs = kiwi.db.app_prefs().map_err(|e| e.to_string())?;
    let cached: HashMap<String, StoreRow> = kiwi
        .db
        .store_rows(&country)
        .map_err(|e| e.to_string())?
        .into_iter()
        .map(|r| (r.bundle_id.clone(), r))
        .collect();
    let auto = OriginalChoice::Auto;
    let mut seen = HashSet::new();
    let mut due = Vec::new();
    for app in kiwi.catalog.apps(None, None) {
        let a = app.app();
        let choice = prefs
            .get(&(app.source.id.clone(), a.bundle_id.clone()))
            .map_or(&auto, |p| &p.original);
        let Some(id) = wanted_id(&a.bundle_id, choice) else {
            continue;
        };
        if seen.insert(id.to_owned()) && cached.get(id).is_none_or(|r| stale(r, now)) {
            due.push(id.to_owned());
        }
    }
    // Kiwi Builds shows apps it can build before they are in any catalogue.
    for recipe in crate::automation::recipes() {
        let id = &recipe.bundle_id;
        if seen.insert(id.clone()) && cached.get(id).is_none_or(|r| stale(r, now)) {
            due.push(id.clone());
        }
    }
    if due.is_empty() {
        return Ok(0);
    }
    match lookup(kiwi, &country, &due).await {
        Ok((found, missing)) => {
            *kiwi.store.failed_at.lock().unwrap() = None;
            kiwi.db
                .save_store_lookup(&country, &found, &missing, Utc::now())
                .map_err(|e| e.to_string())?;
            tracing::info!(
                country,
                found = found.len(),
                missing = missing.len(),
                "looked up App Store apps"
            );
            Ok(due.len())
        }
        Err(e) => {
            *kiwi.store.failed_at.lock().unwrap() = Some(Instant::now());
            Err(e)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_names_drop_taglines() {
        assert_eq!(short_name("Spotify: Music and Podcasts"), "Spotify");
        assert_eq!(short_name("Minecraft: Play with Friends!"), "Minecraft");
        assert_eq!(
            short_name("Elgato Stream Deck Mobile"),
            "Elgato Stream Deck Mobile"
        );
        assert_eq!(short_name("Threads - Share Ideas"), "Threads");
        // A one-letter name keeps its tagline rather than becoming that letter.
        assert_eq!(short_name("X"), "X");
        assert_eq!(short_name("X: Posts | News"), "X: Posts | News");
    }

    #[test]
    fn parses_lookup_results() {
        let body = br#"{"resultCount":3,"results":[
            {"kind":"software","bundleId":"com.google.ios.youtube","trackId":544007664,
             "trackName":"YouTube","artistName":"Google","sellerName":"Google LLC",
             "version":"21.43.5","currentVersionReleaseDate":"2026-10-27T15:00:09Z",
             "artworkUrl100":"https://is1-ssl.mzstatic.com/100.jpg",
             "artworkUrl512":"https://is1-ssl.mzstatic.com/512.jpg",
             "trackViewUrl":"https://apps.apple.com/nl/app/youtube/id544007664?uo=4"},
            {"kind":"mac-software","bundleId":"com.example.mac","trackId":1,"trackName":"Mac",
             "version":"1.0","trackViewUrl":"https://apps.apple.com/nl/app/mac/id1"},
            {"kind":"software","bundleId":"com.example.broken","trackName":"No id",
             "version":"1.0","trackViewUrl":"https://apps.apple.com/nl/app/x/id2"}
        ]}"#;
        let apps = parse_results(body).unwrap();
        assert_eq!(
            apps,
            vec![StoreApp {
                bundle_id: "com.google.ios.youtube".into(),
                track_id: 544007664,
                name: "YouTube".into(),
                developer: Some("Google".into()),
                version: "21.43.5".into(),
                released_at: Some("2026-10-27T15:00:09Z".parse().unwrap()),
                icon_url: Some("https://is1-ssl.mzstatic.com/512.jpg".into()),
                store_url: "https://apps.apple.com/nl/app/youtube/id544007664".into(),
            }]
        );
        assert!(parse_results(b"<html>").is_err());
        assert!(parse_results(b"{}").unwrap().is_empty());
    }

    #[test]
    fn urls_keep_the_base_path() {
        let store = Store::new("http://127.0.0.1:9/itunes/").unwrap();
        assert_eq!(
            store
                .url("lookup", &[("bundleId", "a.b,c.d"), ("country", "nl")])
                .as_str(),
            "http://127.0.0.1:9/itunes/lookup?bundleId=a.b%2Cc.d&country=nl"
        );
        let store = Store::new(DEFAULT_URL).unwrap();
        assert_eq!(
            store.url("search", &[("term", "you tube")]).as_str(),
            "https://itunes.apple.com/search?term=you+tube"
        );
    }

    #[test]
    fn found_apps_go_stale_sooner_than_missing_ones() {
        let now = Utc::now();
        let row = |found: bool, hours: i64| StoreRow {
            bundle_id: "a".into(),
            app: found.then(|| StoreApp {
                bundle_id: "a".into(),
                track_id: 1,
                name: "A".into(),
                developer: None,
                version: "1".into(),
                released_at: None,
                icon_url: None,
                store_url: "https://apps.apple.com/app/id1".into(),
            }),
            checked_at: now - chrono::Duration::hours(hours),
        };
        assert!(!stale(&row(true, 11), now));
        assert!(stale(&row(true, 12), now));
        assert!(!stale(&row(false, 24 * 6), now));
        assert!(stale(&row(false, 24 * 7), now));
    }
}
