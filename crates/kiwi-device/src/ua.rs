use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UaSource {
    /// Safari/WebKit: `Version/` approximates the OS version (major.minor).
    Safari,
    /// The iOS OTA installer (`itunesstored`/`appstored`/...): exact OS version and model.
    Installer,
    /// An Android browser: the platform, and the OS version when the browser reports it.
    Android,
    /// Kiwi's own Android app: the platform and the exact Android release.
    #[serde(rename = "android_app")]
    AndroidApp,
}

impl UaSource {
    /// Matches the `os_version_source` names of the API ("safari", "installer", "android",
    /// "android_app").
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Safari => "safari",
            Self::Installer => "installer",
            Self::Android => "android",
            Self::AndroidApp => "android_app",
        }
    }

    /// Whether the client runs on Android (a browser or Kiwi's app).
    pub fn android(self) -> bool {
        matches!(self, Self::Android | Self::AndroidApp)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct UaInfo {
    pub os_version: Option<String>,
    /// Hardware identifier, e.g. "iPhone19,4" (installer only); [`ANDROID_MODEL`] for every
    /// Android browser, whose User-Agent names no reliable hardware.
    pub model: Option<String>,
    /// OS build, e.g. "24A434" (installer only).
    pub build: Option<String>,
    pub source: UaSource,
}

/// The model recorded for Android devices.
pub const ANDROID_MODEL: &str = "Android";

/// The product token of Kiwi's Android app.
const NATIVE_ANDROID: &str = "Kiwi-Android/";

/// Extracts the iOS version/model from an iOS Safari or OTA installer User-Agent, or the
/// platform from an Android browser's or Kiwi's Android app's. Returns `None` for other
/// clients or when nothing useful is present.
pub fn parse_user_agent(ua: &str) -> Option<UaInfo> {
    let ua = ua.trim();
    if ua.starts_with("Mozilla/") {
        safari(ua).or_else(|| android(ua))
    } else if ua.starts_with(NATIVE_ANDROID) {
        native_android(ua)
    } else {
        installer(ua)
    }
}

/// Whether the request comes from Kiwi's Android app rather than a browser.
pub fn is_native_android(ua: &str) -> bool {
    ua.trim().starts_with(NATIVE_ANDROID)
}

/// The platform a User-Agent comes from: `"ios"` for iOS Safari, other iOS browsers and the
/// OTA installer (also when they tell no version), `"android"` for Android browsers and
/// Kiwi's Android app; `None` for desktop browsers and other clients.
pub fn user_agent_platform(ua: &str) -> Option<&'static str> {
    let ua = ua.trim();
    if let Some(info) = parse_user_agent(ua) {
        return Some(if info.source.android() {
            "android"
        } else {
            "ios"
        });
    }
    let ios = ua.starts_with("Mozilla/")
        && ["(iPhone;", "(iPad;", "(iPod;", "(iPod touch;"]
            .iter()
            .any(|d| ua.contains(d));
    ios.then_some("ios")
}

/// `Kiwi-Android/1.2.0 (12; Android 15; SDK 35; Google Pixel 9a)`: Kiwi's Android app with
/// its version name and code, the Android release, the SDK level and the hardware. The
/// release is the OS version; the hardware is not kept (see [`ANDROID_MODEL`]).
fn native_android(ua: &str) -> Option<UaInfo> {
    let rest = ua.strip_prefix(NATIVE_ANDROID)?;
    let comment = rest
        .split_once('(')
        .and_then(|(_, c)| c.split_once(')'))
        .map_or("", |(c, _)| c);
    let version = comment
        .split(';')
        .map(str::trim)
        .find_map(|p| p.strip_prefix("Android "))
        .map(str::trim)
        .filter(|v| is_version(v));
    Some(UaInfo {
        os_version: version.map(str::to_owned),
        model: Some(ANDROID_MODEL.to_owned()),
        build: None,
        source: UaSource::AndroidApp,
    })
}

/// `Mozilla/5.0 (Linux; Android 10; K) AppleWebKit/537.36 ... Chrome/141.0.0.0 Mobile Safari/537.36`
///
/// Chrome and the browsers built on it always report `Android 10; K`, whatever the phone
/// runs, so that pair says nothing about the OS version. Firefox reports the real one.
fn android(ua: &str) -> Option<UaInfo> {
    let (_, rest) = ua.split_once('(')?;
    let (comment, _) = rest.split_once(')')?;
    let parts: Vec<&str> = comment.split(';').map(str::trim).collect();
    let index = parts
        .iter()
        .position(|p| *p == "Android" || p.starts_with("Android "))?;
    let version = parts[index]
        .strip_prefix("Android ")
        .map(str::trim)
        .filter(|v| is_version(v));
    let frozen = version == Some("10") && parts.get(index + 1) == Some(&"K");
    Some(UaInfo {
        os_version: version.filter(|_| !frozen).map(str::to_owned),
        model: Some(ANDROID_MODEL.to_owned()),
        build: None,
        source: UaSource::Android,
    })
}

/// `itunesstored/1.0 iOS/27.0.1 model/iPhone19,4 hwp/t8150 build/24A434 (6; dt:332) AMS/1`
fn installer(ua: &str) -> Option<UaInfo> {
    let tokens: Vec<&str> = ua
        .split(|c: char| c.is_whitespace() || matches!(c, '(' | ')' | ';'))
        .filter(|t| !t.is_empty())
        .collect();
    let mut info = UaInfo {
        os_version: None,
        model: None,
        build: None,
        source: UaSource::Installer,
    };
    for (i, token) in tokens.iter().enumerate() {
        let (key, value) = match token.split_once('/') {
            Some(kv) => kv,
            // "iOS 27.0.1" inside a comment
            None => (*token, tokens.get(i + 1).copied().unwrap_or_default()),
        };
        let slot = match key.to_ascii_lowercase().as_str() {
            "ios" | "ipados" if is_version(value) => &mut info.os_version,
            "model" if is_model(value) => &mut info.model,
            "build" if is_build(value) => &mut info.build,
            _ => continue,
        };
        slot.get_or_insert_with(|| value.to_owned());
    }
    (info.os_version.is_some() || info.model.is_some()).then_some(info)
}

/// `Mozilla/5.0 (iPhone; CPU iPhone OS 18_6 like Mac OS X) ... Version/27.0 Mobile/15E148 Safari/604.1`
///
/// Since iOS 26 the `OS 18_6` token is frozen, so `Version/` is used. Older releases report
/// the exact version in the OS token, which is preferred when it agrees with `Version/`.
fn safari(ua: &str) -> Option<UaInfo> {
    if !["(iPhone", "(iPad", "(iPod"].iter().any(|d| ua.contains(d)) {
        return None;
    }
    let version = ua
        .split_whitespace()
        .find_map(|t| t.strip_prefix("Version/"))
        .filter(|v| is_version(v));
    let os_token = ua
        .split_once(" OS ")
        .and_then(|(_, rest)| rest.split_whitespace().next())
        .filter(|t| *t != "18_6")
        .map(|t| t.replace('_', "."))
        .filter(|v| is_version(v));
    let os_version = match (version, os_token) {
        (Some(v), Some(os)) if major_minor(v) == major_minor(&os) => Some(os),
        (Some(v), _) => Some(v.to_owned()),
        (None, os) => os,
    }?;
    Some(UaInfo {
        os_version: Some(os_version),
        model: None,
        build: None,
        source: UaSource::Safari,
    })
}

fn major_minor(v: &str) -> (&str, &str) {
    let mut parts = v.split('.');
    (
        parts.next().unwrap_or_default(),
        parts.next().unwrap_or("0"),
    )
}

/// 1 to 4 numeric components, e.g. "27", "27.0.1".
fn is_version(v: &str) -> bool {
    let parts: Vec<&str> = v.split('.').collect();
    (1..=4).contains(&parts.len())
        && parts
            .iter()
            .all(|p| (1..=4).contains(&p.len()) && p.bytes().all(|b| b.is_ascii_digit()))
}

/// e.g. "iPhone19,4", "iPad16,3".
fn is_model(v: &str) -> bool {
    let Some((head, minor)) = v.split_once(',') else {
        return false;
    };
    let digits = head.trim_start_matches(|c: char| c.is_ascii_alphabetic());
    digits.len() < head.len()
        && v.len() <= 32
        && !digits.is_empty()
        && digits.bytes().all(|b| b.is_ascii_digit())
        && !minor.is_empty()
        && minor.bytes().all(|b| b.is_ascii_digit())
}

/// e.g. "24A434", "23B5044a".
fn is_build(v: &str) -> bool {
    (3..=16).contains(&v.len())
        && v.bytes().all(|b| b.is_ascii_alphanumeric())
        && v.bytes().next().is_some_and(|b| b.is_ascii_digit())
}
