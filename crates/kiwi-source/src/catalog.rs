use crate::{Error, Result, Url, relative_url};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    cmp::Ordering,
    collections::{BTreeSet, HashSet},
};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Catalog {
    pub name: String,
    #[serde(default)]
    pub identifier: Option<String>,
    #[serde(default)]
    pub source_url: Option<String>,
    #[serde(default)]
    pub subtitle: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub icon_url: Option<Url>,
    pub apps: Vec<App>,
    #[serde(default)]
    pub diagnostics: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct App {
    pub name: String,
    pub bundle_id: String,
    #[serde(default)]
    pub developer: Option<String>,
    #[serde(default)]
    pub subtitle: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub icon_url: Option<Url>,
    #[serde(default)]
    pub screenshots: Vec<Url>,
    /// `#RRGGBB`.
    #[serde(default)]
    pub tint_color: Option<String>,
    #[serde(default)]
    pub category: Option<String>,
    #[serde(default)]
    pub permissions: Permissions,
    /// Source order, newest first, as defined by the Classic specification.
    pub releases: Vec<Release>,
    #[serde(default)]
    pub unavailable: Option<String>,
}

/// What the publisher declares the app requests. Informational: signing uses the binary.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Permissions {
    #[serde(default)]
    pub entitlements: Vec<String>,
    /// (usage key, description), e.g. `("NSCameraUsageDescription", "...")`. Legacy
    /// `permissions` entries use their `type` (e.g. `"camera"`) as the key.
    #[serde(default)]
    pub privacy: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Release {
    pub version: String,
    #[serde(default)]
    pub build: Option<String>,
    /// Local packaging revision; never substituted for the app's own version.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub revision: Option<u64>,
    #[serde(default)]
    pub date: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
    pub download_url: Url,
    #[serde(default)]
    pub size: Option<u64>,
    #[serde(default)]
    pub sha256: Option<String>,
    #[serde(default)]
    pub min_os: Option<String>,
    #[serde(default)]
    pub max_os: Option<String>,
    #[serde(default)]
    pub platforms: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceProfile {
    pub os_version: String,
    pub platform: String,
    /// UIDeviceFamily: 1 = iPhone/iPod, 2 = iPad, 3 = TV, 4 = Watch.
    pub family: u64,
}

fn field(v: &Value, key: &str) -> Option<String> {
    v.get(key)
        .and_then(Value::as_str)
        .filter(|s| !s.trim().is_empty())
        .map(str::to_owned)
}
fn required(v: &Value, key: &str) -> Result<String> {
    field(v, key).ok_or_else(|| Error::new("Source parse", format!("Missing/invalid {key}")))
}
fn optional_url(v: &Value, key: &str, base: &Url, warnings: &mut Vec<String>) -> Option<Url> {
    field(v, key).and_then(|s| match relative_url(base, &s) {
        Ok(u) => Some(u),
        Err(e) => {
            warnings.push(format!("Ignoring {key}: {e}"));
            None
        }
    })
}

/// Deepest array/object nesting the JSON5 fallback parses (serde_json stops at 128 too).
const MAX_JSON5_DEPTH: usize = 128;

/// Whether JSON5 text nests arrays and objects more than `limit` deep. Strings (both quote
/// styles, with escapes) and comments are skipped as the JSON5 grammar defines them, so
/// brackets the parser would read are never hidden from the count.
fn json5_depth_exceeds(input: &str, limit: usize) -> bool {
    let mut chars = input.chars();
    let mut depth = 0usize;
    while let Some(c) = chars.next() {
        match c {
            '[' | '{' => {
                depth += 1;
                if depth > limit {
                    return true;
                }
            }
            ']' | '}' => depth = depth.saturating_sub(1),
            '"' | '\'' => {
                while let Some(s) = chars.next() {
                    if s == '\\' {
                        chars.next();
                    } else if s == c {
                        break;
                    }
                }
            }
            '/' => match chars.clone().next() {
                Some('/') => {
                    for s in chars.by_ref() {
                        if matches!(s, '\n' | '\r' | '\u{2028}' | '\u{2029}') {
                            break;
                        }
                    }
                }
                Some('*') => {
                    chars.next();
                    let mut star = false;
                    for s in chars.by_ref() {
                        if star && s == '/' {
                            break;
                        }
                        star = s == '*';
                    }
                }
                _ => {}
            },
            _ => {}
        }
    }
    false
}

pub fn parse_catalog(bytes: &[u8], base: &Url) -> Result<Catalog> {
    let input = std::str::from_utf8(bytes)
        .map_err(|e| Error::new("Source parse", e))?
        .trim_start_matches('\u{feff}');
    if input.trim_start().starts_with('<') {
        return Err(Error::new(
            "Source parse",
            "Received HTML/XML instead of a JSON source; use the raw file URL",
        ));
    }
    let value: Value = serde_json::from_str(input).or_else(|_| {
        // The JSON5 parser recurses without a limit: a deeply nested feed would
        // overflow the stack and abort the process.
        if json5_depth_exceeds(input, MAX_JSON5_DEPTH) {
            return Err(Error::new(
                "Source parse",
                format!("Arrays and objects are nested more than {MAX_JSON5_DEPTH} levels deep"),
            ));
        }
        json5::from_str(input).map_err(|e| Error::new("Source parse", e))
    })?;
    let name = required(&value, "name")?;
    let apps = value
        .get("apps")
        .and_then(Value::as_array)
        .ok_or_else(|| Error::new("Source parse", "Missing/invalid apps array"))?;
    if apps.len() > 10_000 {
        return Err(Error::new("Source parse", "Catalog exceeds 10,000 apps"));
    }
    let mut diagnostics = vec![];
    let mut valid_apps = vec![];
    let mut ids = HashSet::new();
    for (i, app) in apps.iter().enumerate() {
        match parse_app(app, base, &mut diagnostics) {
            Ok(app) => {
                if ids.insert(app.bundle_id.clone()) {
                    valid_apps.push(app);
                } else {
                    diagnostics.push(format!(
                        "App {}: duplicate bundleIdentifier {} in this source; ignored",
                        i + 1,
                        app.bundle_id
                    ));
                }
            }
            Err(e) => diagnostics.push(format!("App {}: {e}", i + 1)),
        }
    }
    if !apps.is_empty() && valid_apps.is_empty() {
        return Err(Error::new(
            "Source parse",
            format!("No valid apps. {}", diagnostics.join("; ")),
        ));
    }
    if apps.is_empty() {
        diagnostics.push("This source has an empty catalog.".into());
    }
    let icon_url = optional_url(&value, "iconURL", base, &mut diagnostics);
    Ok(Catalog {
        name,
        identifier: field(&value, "identifier"),
        source_url: field(&value, "sourceURL"),
        subtitle: field(&value, "subtitle"),
        description: field(&value, "description"),
        icon_url,
        apps: valid_apps,
        diagnostics,
    })
}

fn parse_app(v: &Value, base: &Url, warnings: &mut Vec<String>) -> Result<App> {
    let name = required(v, "name")?;
    let bundle_id = required(v, "bundleIdentifier")?;
    if !valid_bundle_id(&bundle_id) {
        return Err(Error::new(
            "Source parse",
            format!("{name}: invalid bundleIdentifier {bundle_id}"),
        ));
    }
    let mut releases = vec![];
    if let Some(versions) = v.get("versions").and_then(Value::as_array) {
        if versions.len() > 2_000 {
            return Err(Error::new("Source parse", "More than 2,000 versions"));
        }
        for (i, version) in versions.iter().enumerate() {
            match parse_release(version, base) {
                Ok(r) => {
                    if !releases.iter().any(|x: &Release| {
                        x.version == r.version
                            && x.build == r.build
                            && x.download_url == r.download_url
                            && x.sha256 == r.sha256
                    }) {
                        releases.push(r);
                    }
                }
                Err(e) => warnings.push(format!("{name}, release {}: {e}", i + 1)),
            }
        }
    }
    if releases.is_empty()
        && let Ok(mut r) = parse_release(v, base)
    {
        // On a legacy entry, localizedDescription describes the app, not the release.
        r.notes = field(v, "versionDescription");
        if v.get("versions").is_some() {
            warnings.push(format!(
                "{name}: using legacy flat version/downloadURL fields (no valid versions record)."
            ));
        }
        releases.push(r);
    }
    let unavailable = if v.get("patreon").is_some_and(|p| !p.is_null()) {
        Some("Paid-source authentication is unsupported.".into())
    } else if v.get("marketplaceID").is_some() {
        Some("AltStore PAL marketplace packages are unsupported.".into())
    } else if releases.is_empty() {
        Some("No valid IPA releases. Check this source's diagnostics.".into())
    } else {
        None
    };
    if releases.is_empty() {
        warnings.push(format!("{name}: no valid IPA releases"));
    }
    let icon_url = optional_url(v, "iconURL", base, warnings);
    let mut screenshots = vec![];
    fn collect(v: &Value, base: &Url, out: &mut Vec<Url>) {
        if out.len() >= 50 {
            return;
        }
        match v {
            Value::String(s) => {
                if let Ok(u) = relative_url(base, s) {
                    out.push(u);
                }
            }
            Value::Array(a) => {
                for s in a {
                    collect(s, base, out);
                }
            }
            Value::Object(o) => {
                if let Some(s) = o.get("imageURL") {
                    collect(s, base, out);
                } else {
                    for key in ["iphone", "ipad"] {
                        if let Some(a) = o.get(key) {
                            collect(a, base, out);
                        }
                    }
                }
            }
            _ => {}
        }
    }
    if let Some(s) = v.get("screenshots").or_else(|| v.get("screenshotURLs")) {
        collect(s, base, &mut screenshots);
    }
    let permissions = parse_permissions(v, &name, warnings);
    Ok(App {
        name,
        bundle_id,
        developer: field(v, "developerName"),
        subtitle: field(v, "subtitle"),
        description: field(v, "localizedDescription").or_else(|| field(v, "description")),
        icon_url,
        screenshots,
        tint_color: field(v, "tintColor").and_then(|s| tint_color(&s)),
        category: field(v, "category"),
        permissions,
        releases,
        unavailable,
    })
}

/// `RRGGBB`, `#RRGGBB` or `#RGB` to `#RRGGBB`.
fn tint_color(value: &str) -> Option<String> {
    let hex = value.trim();
    let hex = hex.strip_prefix('#').unwrap_or(hex);
    if !hex.bytes().all(|b| b.is_ascii_hexdigit()) {
        return None;
    }
    let hex = match hex.len() {
        6 => hex.to_ascii_uppercase(),
        3 => hex
            .chars()
            .flat_map(|c| [c, c])
            .collect::<String>()
            .to_ascii_uppercase(),
        _ => return None,
    };
    Some(format!("#{hex}"))
}

/// Current `appPermissions` (entitlements as strings or `{name}`; privacy as a map or
/// `[{name, usageDescription}]`) and legacy `permissions: [{type, usageDescription}]`.
/// Malformed entries are skipped with one diagnostic per app.
fn parse_permissions(v: &Value, app: &str, warnings: &mut Vec<String>) -> Permissions {
    fn text(v: &Value) -> Option<&str> {
        Some(v.as_str()?.trim()).filter(|s| !s.is_empty())
    }
    /// The first description for a key wins. False when either part is missing.
    fn privacy(out: &mut Permissions, key: Option<&str>, description: Option<&Value>) -> bool {
        let (Some(key), Some(description)) = (key, description.and_then(Value::as_str)) else {
            return false;
        };
        if !out.privacy.iter().any(|(k, _)| k == key) {
            out.privacy
                .push((key.to_owned(), description.trim().to_owned()));
        }
        true
    }
    let mut out = Permissions::default();
    let mut malformed = BTreeSet::new();
    match v.get("appPermissions") {
        None | Some(Value::Null) => {}
        Some(Value::Object(permissions)) => {
            match permissions.get("entitlements") {
                None | Some(Value::Null) => {}
                Some(Value::Array(items)) => {
                    for item in items {
                        match text(item).or_else(|| item.get("name").and_then(text)) {
                            Some(e) if !out.entitlements.iter().any(|x| x == e) => {
                                out.entitlements.push(e.to_owned())
                            }
                            Some(_) => {}
                            None => _ = malformed.insert("appPermissions.entitlements"),
                        }
                    }
                }
                Some(_) => _ = malformed.insert("appPermissions.entitlements"),
            }
            match permissions.get("privacy") {
                None | Some(Value::Null) => {}
                Some(Value::Object(map)) => {
                    for (key, description) in map {
                        let key = Some(key.trim()).filter(|k| !k.is_empty());
                        if !privacy(&mut out, key, Some(description)) {
                            malformed.insert("appPermissions.privacy");
                        }
                    }
                }
                Some(Value::Array(items)) => {
                    for item in items {
                        let key = item.get("name").and_then(text);
                        if !privacy(&mut out, key, item.get("usageDescription")) {
                            malformed.insert("appPermissions.privacy");
                        }
                    }
                }
                Some(_) => _ = malformed.insert("appPermissions.privacy"),
            }
        }
        Some(_) => _ = malformed.insert("appPermissions"),
    }
    match v.get("permissions") {
        None | Some(Value::Null) => {}
        Some(Value::Array(items)) => {
            for item in items {
                let key = item.get("type").and_then(text);
                if !privacy(&mut out, key, item.get("usageDescription")) {
                    malformed.insert("permissions");
                }
            }
        }
        Some(_) => _ = malformed.insert("permissions"),
    }
    if !malformed.is_empty() {
        warnings.push(format!(
            "{app}: ignored malformed {}",
            malformed.into_iter().collect::<Vec<_>>().join(", ")
        ));
    }
    out
}

fn parse_release(v: &Value, base: &Url) -> Result<Release> {
    let version = required(v, "version")?;
    let download_url = relative_url(base, &required(v, "downloadURL")?)?;
    let sha256 = field(v, "sha256").map(|s| s.to_ascii_lowercase());
    if sha256
        .as_ref()
        .is_some_and(|s| s.len() != 64 || !s.bytes().all(|b| b.is_ascii_hexdigit()))
    {
        return Err(Error::new(
            "Source parse",
            "Invalid sha256; expected 64 hexadecimal characters",
        ));
    }
    let min_os = field(v, "minOSVersion");
    let max_os = field(v, "maxOSVersion");
    if min_os
        .iter()
        .chain(max_os.iter())
        .any(|s| numeric_version(s).is_none())
    {
        return Err(Error::new("Source parse", "Invalid OS version requirement"));
    }
    if let (Some(min), Some(max)) = (&min_os, &max_os)
        && compare_numeric(min, max) == Some(Ordering::Greater)
    {
        return Err(Error::new(
            "Source parse",
            "minOSVersion exceeds maxOSVersion",
        ));
    }
    let platforms = match v.get("platforms").or_else(|| v.get("platform")) {
        Some(Value::Array(a)) => a
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        Some(Value::String(s)) => vec![s.clone()],
        _ => vec![],
    };
    Ok(Release {
        version,
        build: field(v, "buildVersion"),
        revision: None,
        date: field(v, "date").or_else(|| field(v, "versionDate")),
        notes: field(v, "localizedDescription").or_else(|| field(v, "versionDescription")),
        download_url,
        size: v.get("size").and_then(|s| {
            s.as_u64()
                .or_else(|| s.as_str().and_then(|s| s.parse().ok()))
        }),
        sha256,
        min_os,
        max_os,
        platforms,
    })
}

pub fn valid_bundle_id(s: &str) -> bool {
    !s.is_empty()
        && s.len() <= 255
        && s.contains('.')
        && s.split('.')
            .all(|s| !s.is_empty() && s.bytes().all(|c| c.is_ascii_alphanumeric() || c == b'-'))
}

fn numeric_version(v: &str) -> Option<Vec<u64>> {
    if v.is_empty() {
        return None;
    }
    v.split('.')
        .map(|p| {
            if !p.is_empty() && p.bytes().all(|c| c.is_ascii_digit()) {
                p.parse().ok()
            } else {
                None
            }
        })
        .collect()
}
pub fn compare_numeric(a: &str, b: &str) -> Option<Ordering> {
    let a = numeric_version(a)?;
    let b = numeric_version(b)?;
    Some(
        (0..a.len().max(b.len()))
            .map(|i| a.get(i).unwrap_or(&0).cmp(b.get(i).unwrap_or(&0)))
            .find(|c| *c != Ordering::Equal)
            .unwrap_or(Ordering::Equal),
    )
}

pub fn platform_matches(platform: &str, device: &str) -> bool {
    fn normalized(p: &str) -> String {
        match p.to_ascii_lowercase().as_str() {
            "iphoneos" | "ios" | "ipados" => "ios".into(),
            "appletvos" | "tvos" => "tvos".into(),
            "watchos" => "watchos".into(),
            "macosx" | "macos" => "macos".into(),
            _ => p.to_ascii_lowercase(),
        }
    }
    normalized(platform) == normalized(device)
}
impl Release {
    pub fn incompatibility(&self, device: &DeviceProfile) -> Option<String> {
        if !self.platforms.is_empty()
            && !self
                .platforms
                .iter()
                .any(|p| platform_matches(p, &device.platform))
        {
            return Some(format!(
                "Requires platform {} (selected: {})",
                self.platforms.join(", "),
                device.platform
            ));
        }
        if let Some(min) = &self.min_os {
            match compare_numeric(&device.os_version, min) {
                Some(Ordering::Less) => {
                    return Some(format!(
                        "Requires OS {min}+ (selected: {})",
                        device.os_version
                    ));
                }
                None => return Some("Unable to determine device OS version".into()),
                _ => {}
            }
        }
        if let Some(max) = &self.max_os {
            match compare_numeric(&device.os_version, max) {
                Some(Ordering::Greater) => {
                    return Some(format!(
                        "Requires OS {max} or earlier (selected: {})",
                        device.os_version
                    ));
                }
                None => return Some("Unable to determine device OS version".into()),
                _ => {}
            }
        }
        None
    }
    pub fn same_release(&self, other: &Self) -> bool {
        self.version == other.version
            && self.build == other.build
            && self.revision == other.revision
    }
    pub fn same_artifact(&self, other: &Self) -> bool {
        match (&self.sha256, &other.sha256) {
            (Some(a), Some(b)) => a == b,
            _ => self.download_url == other.download_url,
        }
    }
}
impl App {
    pub fn newest_eligible(&self, device: Option<&DeviceProfile>) -> Option<usize> {
        if self.unavailable.is_some() {
            return None;
        }
        self.releases
            .iter()
            .position(|r| device.is_none_or(|d| r.incompatibility(d).is_none()))
    }
    pub fn matches(&self, query: &str) -> bool {
        let q = query.to_lowercase();
        [
            &self.name,
            &self.bundle_id,
            self.developer.as_deref().unwrap_or(""),
        ]
        .iter()
        .any(|s| s.to_lowercase().contains(&q))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReleaseChange {
    Same,
    Newer,
    Older,
    Ambiguous,
}

/// Dates never imply an update. Numeric versions/builds prevent downgrades. For nonnumeric
/// releases, use publisher ordering only while the installed release is still in history.
pub fn release_change(app: &App, candidate: usize, installed: &Release) -> ReleaseChange {
    let Some(new) = app.releases.get(candidate) else {
        return ReleaseChange::Ambiguous;
    };
    if new.same_release(installed) {
        if !new.same_artifact(installed) {
            return ReleaseChange::Ambiguous;
        }
        return ReleaseChange::Same;
    }
    if new.version == installed.version && new.build == installed.build {
        return match (new.revision, installed.revision) {
            (Some(a), Some(b)) if a > b => ReleaseChange::Newer,
            (Some(a), Some(b)) if a < b => ReleaseChange::Older,
            _ => ReleaseChange::Ambiguous,
        };
    }
    let cmp = compare_numeric(&new.version, &installed.version);
    let cmp = if cmp == Some(Ordering::Equal) {
        match (&new.build, &installed.build) {
            (Some(a), Some(b)) => compare_numeric(a, b),
            _ => None,
        }
    } else {
        cmp
    };
    match cmp {
        Some(Ordering::Greater) => ReleaseChange::Newer,
        Some(Ordering::Less) => ReleaseChange::Older,
        Some(Ordering::Equal) => ReleaseChange::Same,
        None => match app
            .releases
            .iter()
            .position(|r| r.same_release(installed) && r.same_artifact(installed))
        {
            Some(i) if candidate < i => ReleaseChange::Newer,
            Some(i) if candidate > i => ReleaseChange::Older,
            _ => ReleaseChange::Ambiguous,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json5_depth_skips_strings_and_comments() {
        let nested = |n| format!("{}{}", "[".repeat(n), "]".repeat(n));
        assert!(!json5_depth_exceeds(&nested(3), 3));
        assert!(json5_depth_exceeds(&nested(4), 3));
        assert!(json5_depth_exceeds("{a:{b:[[", 3));
        // Brackets in strings and comments do not nest.
        assert!(!json5_depth_exceeds(
            r#"["[[[[", '{{{{', "\"[[[[", '\'{{{{']"#,
            1
        ));
        assert!(!json5_depth_exceeds("[/* [[[[ */ // [[[[\n]", 1));
        // ...but a quote in a comment does not start a string, and line comments end at
        // every JSON5 line terminator.
        assert!(json5_depth_exceeds("// it's\n[[[[ '", 3));
        assert!(json5_depth_exceeds("// x\r[[[[", 3));
        assert!(json5_depth_exceeds("// x\u{2028}[[[[", 3));
        assert!(json5_depth_exceeds("/* '*/[[[[", 3));
        assert!(json5_depth_exceeds("/*/ '*/[[[[", 3));
    }
}
