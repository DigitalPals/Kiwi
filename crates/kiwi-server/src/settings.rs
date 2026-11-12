use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use crate::error::ApiError;

/// User preferences (`Settings` in api.md), stored as one JSON value per key.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub refresh_interval_minutes: u32,
    pub auto_prepare_updates: bool,
    pub notify_updates: bool,
    pub notify_expiry: bool,
    pub devicectl_sync: bool,
    pub devicectl_sync_minutes: u32,
    pub strip_watch_apps: bool,
    /// Storefront (ISO 3166-1 alpha-2, lowercase) whose App Store versions patched apps are
    /// compared with.
    pub app_store_country: String,
    /// Kiwi Builds: patch App Store apps on the Mac. Off until the owner turns it on.
    pub kiwi_builds: bool,
    /// Kiwi Builds follows new releases of the iOS apps' tweaks by itself (`automation::follow`);
    /// off, builds use the sources this Kiwi version pins.
    pub follow_tweak_updates: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            refresh_interval_minutes: 60,
            auto_prepare_updates: true,
            notify_updates: true,
            notify_expiry: true,
            devicectl_sync: true,
            devicectl_sync_minutes: 10,
            strip_watch_apps: true,
            app_store_country: "us".into(),
            kiwi_builds: false,
            follow_tweak_updates: true,
        }
    }
}

const REFRESH_RANGE: (u32, u32) = (15, 7 * 24 * 60);
const SYNC_RANGE: (u32, u32) = (5, 24 * 60);

impl Settings {
    /// Builds settings from stored key/value pairs over `defaults`; unknown keys and invalid
    /// values fall back to them.
    pub fn from_pairs(defaults: Self, pairs: impl IntoIterator<Item = (String, Value)>) -> Self {
        let mut settings = defaults;
        for (key, value) in pairs {
            let mut patch = Map::new();
            patch.insert(key, value);
            if let Ok(next) = settings.patched(&patch) {
                settings = next;
            }
        }
        settings
    }

    /// Applies a partial update. Unknown keys, wrong types and out-of-range values are
    /// rejected with a user-readable message.
    pub fn patched(&self, patch: &Map<String, Value>) -> Result<Self, ApiError> {
        let mut next = self.clone();
        for (key, value) in patch {
            let flag = || {
                value
                    .as_bool()
                    .ok_or_else(|| ApiError::bad_request(format!("{key} must be true or false")))
            };
            let minutes = |(min, max): (u32, u32)| {
                value
                    .as_u64()
                    .filter(|m| (u64::from(min)..=u64::from(max)).contains(m))
                    .map(|m| m as u32)
                    .ok_or_else(|| {
                        ApiError::bad_request(format!(
                            "{key} must be a whole number of minutes from {min} to {max}"
                        ))
                    })
            };
            match key.as_str() {
                "refresh_interval_minutes" => {
                    next.refresh_interval_minutes = minutes(REFRESH_RANGE)?
                }
                "devicectl_sync_minutes" => next.devicectl_sync_minutes = minutes(SYNC_RANGE)?,
                "auto_prepare_updates" => next.auto_prepare_updates = flag()?,
                "notify_updates" => next.notify_updates = flag()?,
                "notify_expiry" => next.notify_expiry = flag()?,
                "devicectl_sync" => next.devicectl_sync = flag()?,
                "strip_watch_apps" => next.strip_watch_apps = flag()?,
                "kiwi_builds" => next.kiwi_builds = flag()?,
                "follow_tweak_updates" => next.follow_tweak_updates = flag()?,
                "app_store_country" => {
                    next.app_store_country = value
                        .as_str()
                        .map(str::trim)
                        .filter(|c| c.len() == 2 && c.bytes().all(|b| b.is_ascii_alphabetic()))
                        .map(str::to_ascii_lowercase)
                        .ok_or_else(|| {
                            ApiError::bad_request(format!(
                                "{key} must be a two-letter country code such as \"nl\""
                            ))
                        })?
                }
                _ => return Err(ApiError::bad_request(format!("Unknown setting {key}"))),
            }
        }
        Ok(next)
    }

    pub fn pairs(&self) -> Vec<(String, Value)> {
        match serde_json::to_value(self) {
            Ok(Value::Object(map)) => map.into_iter().collect(),
            _ => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn patch(v: Value) -> Map<String, Value> {
        v.as_object().unwrap().clone()
    }

    #[test]
    fn defaults_match_the_contract() {
        let s = Settings::default();
        assert_eq!(
            serde_json::to_value(&s).unwrap(),
            json!({"refresh_interval_minutes":60,"auto_prepare_updates":true,"notify_updates":true,
                   "notify_expiry":true,"devicectl_sync":true,"devicectl_sync_minutes":10,"strip_watch_apps":true,
                   "app_store_country":"us","kiwi_builds":false,"follow_tweak_updates":true})
        );
    }

    #[test]
    fn validates_patches() {
        let s = Settings::default();
        let next = s
            .patched(&patch(
                json!({"refresh_interval_minutes": 15, "notify_updates": false}),
            ))
            .unwrap();
        assert_eq!(next.refresh_interval_minutes, 15);
        assert!(!next.notify_updates);
        assert!(next.notify_expiry);
        let us = s
            .patched(&patch(json!({"app_store_country": " US "})))
            .unwrap();
        assert_eq!(us.app_store_country, "us");
        for bad in [
            json!({"refresh_interval_minutes": 14}),
            json!({"refresh_interval_minutes": 30.5}),
            json!({"refresh_interval_minutes": "60"}),
            json!({"devicectl_sync_minutes": 4}),
            json!({"devicectl_sync": "yes"}),
            json!({"kiwi_builds": 1}),
            json!({"app_store_country": "nld"}),
            json!({"app_store_country": "n1"}),
            json!({"app_store_country": null}),
            json!({"unknown": true}),
        ] {
            let err = s.patched(&patch(bad.clone())).unwrap_err();
            assert_eq!(err.code, "bad_request", "{bad}");
        }
        assert_eq!(s.patched(&Map::new()).unwrap(), s);
    }

    #[test]
    fn round_trips_pairs_and_ignores_bad_stored_values() {
        let s = Settings {
            devicectl_sync_minutes: 30,
            strip_watch_apps: false,
            ..Settings::default()
        };
        assert_eq!(Settings::from_pairs(Settings::default(), s.pairs()), s);
        let defaults = Settings {
            app_store_country: "nl".into(),
            ..Settings::default()
        };
        let restored = Settings::from_pairs(
            defaults,
            [
                ("refresh_interval_minutes".to_owned(), json!(1)),
                ("removed_setting".to_owned(), json!(true)),
                ("notify_expiry".to_owned(), json!(false)),
            ],
        );
        assert_eq!(restored.refresh_interval_minutes, 60);
        assert!(!restored.notify_expiry);
        assert_eq!(restored.app_store_country, "nl");
    }
}
