//! The Morphe community index (`bundles.json`): parsed and validated as untrusted data.
//!
//! The index is discovery data only. Kiwi keeps what it shows (app names, categories,
//! icons through the art proxy, patch names for search) and what it ranks bundles by; it
//! never follows a URL from it except an app's icon, and it builds every GitHub address
//! itself from a validated `owner/repo`. Entries that don't pass validation are skipped;
//! an index without the expected shape, or beyond the size limits, is refused as a whole
//! (the last good copy stays in use).
use std::{
    cmp::Ordering,
    collections::{BTreeMap, BTreeSet, HashMap},
};

use serde_json::{Map, Value};

/// Most entries Kiwi accepts of each kind; a larger index is refused.
const MAX_BUNDLES: usize = 5_000;
const MAX_PATCHES: usize = 300_000;
const MAX_PATCHES_PER_BUNDLE: usize = 5_000;
const MAX_STORE: usize = 100_000;
const MAX_COMPATIBILITIES: usize = 100_000;
const MAX_VERSIONS: usize = 500;
const MAX_FEATURED: usize = 1_000;
const MAX_NAME: usize = 120;
const MAX_TEXT: usize = 1_000;

/// One GitHub repository's patch bundle.
#[derive(Debug, Clone)]
pub struct Bundle {
    /// `owner/name`, as GitHub spells it in the index.
    pub repo: String,
    pub author: String,
    pub stars: u64,
    /// Milliseconds since the epoch.
    pub updated_at: Option<i64>,
    /// Per package: when the bundle's patches for it last changed.
    pub app_updates: BTreeMap<String, i64>,
    /// How many apps the bundle patches.
    pub targets: usize,
    pub patches: Vec<Patch>,
}

#[derive(Debug, Clone)]
pub struct Patch {
    pub name: String,
    pub description: String,
    /// Enabled unless the user turns it off (Morphe's `use`; absent means on).
    pub default: bool,
    /// The packages and versions it is for; `None` for a universal patch.
    pub compatibility: Option<usize>,
}

/// One package a patch is for, and the versions it targets (empty: any version).
#[derive(Debug, Clone, PartialEq)]
pub struct Target {
    pub package: String,
    pub name: Option<String>,
    pub versions: Vec<String>,
}

/// An app the index offers patches for.
#[derive(Debug, Clone)]
pub struct App {
    pub package: String,
    pub name: String,
    pub category: Option<String>,
    /// An https URL, shown only through Kiwi's art proxy.
    pub icon_url: Option<String>,
    pub description: Option<String>,
    pub installs: u64,
    pub featured: bool,
    /// Indexes into [`Index::bundles`] of the bundles with patches for this app.
    pub bundles: Vec<usize>,
    /// The most patches any one bundle has for this app.
    pub patches: usize,
    /// Lower-case name and package, then lower-case patch names and descriptions.
    names: String,
    patch_text: String,
}

#[derive(Debug, Clone)]
pub struct Index {
    pub bundles: Vec<Bundle>,
    pub compatibilities: Vec<Vec<Target>>,
    /// By package.
    pub apps: BTreeMap<String, App>,
    /// Featured packages, deduplicated, in index order.
    pub featured: Vec<String>,
}

/// What a bundle offers one app, from the index (not yet checked against its release).
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub bundle: usize,
    pub repo: String,
    pub author: String,
    pub stars: u64,
    pub updated_at: Option<i64>,
    /// Patches for this app (specific to it; universal patches don't count).
    pub patches: usize,
    pub default_patches: usize,
    /// Target versions, newest first; empty when the patches take any version.
    pub versions: Vec<String>,
    pub targets: usize,
}

pub fn valid_package(value: &str) -> bool {
    // ^[a-zA-Z][a-zA-Z0-9_]*(\.[a-zA-Z0-9_]+)+$
    if value.is_empty() || value.len() > 255 {
        return false;
    }
    let mut parts = value.split('.');
    let Some(first) = parts.next() else {
        return false;
    };
    let mut first_chars = first.chars();
    if !first_chars.next().is_some_and(|c| c.is_ascii_alphabetic())
        || !first_chars.all(|c| c.is_ascii_alphanumeric() || c == '_')
    {
        return false;
    }
    let mut more = 0;
    for part in parts {
        if part.is_empty() || !part.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
            return false;
        }
        more += 1;
    }
    more > 0
}

/// A GitHub `owner/name`: an owner of letters, digits and single hyphens (at most 39), a
/// repository name of letters, digits, `.`, `_` and `-` (at most 100, not `.` or `..`).
pub fn valid_repo(value: &str) -> bool {
    let Some((owner, name)) = value.split_once('/') else {
        return false;
    };
    let owner_ok = (1..=39).contains(&owner.len())
        && owner
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        && !owner.starts_with('-')
        && !owner.ends_with('-')
        && !owner.contains("--");
    let name_ok = (1..=100).contains(&name.len())
        && name
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'_' | b'-'))
        && name != "."
        && name != "..";
    owner_ok && name_ok
}

/// Plain text from the index: HTML entities the site leaves in decoded, control
/// characters dropped, whitespace collapsed, at most `max` characters.
pub fn clean_text(value: &str, max: usize) -> String {
    let decoded = value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&amp;", "&");
    let mut out = String::new();
    let mut space = false;
    for c in decoded.chars() {
        if c.is_whitespace() {
            space = !out.is_empty();
            continue;
        }
        // Control and format characters (bidi overrides among them).
        if c.is_control()
            || matches!(c, '\u{200B}'..='\u{200F}' | '\u{202A}'..='\u{202E}' | '\u{2066}'..='\u{2069}' | '\u{FEFF}')
        {
            continue;
        }
        if space {
            out.push(' ');
            space = false;
        }
        out.push(c);
        if out.chars().count() >= max {
            break;
        }
    }
    out
}

fn text(value: Option<&Value>, max: usize) -> Option<String> {
    value
        .and_then(Value::as_str)
        .map(|s| clean_text(s, max))
        .filter(|s| !s.is_empty())
}

fn count(value: Option<&Value>) -> u64 {
    value.and_then(Value::as_u64).unwrap_or(0)
}

/// A time in milliseconds since the epoch between 2000 and 2200.
fn millis(value: Option<&Value>) -> Option<i64> {
    value
        .and_then(Value::as_i64)
        .filter(|t| (946_684_800_000..7_258_118_400_000).contains(t))
}

fn icon(value: Option<&Value>) -> Option<String> {
    let raw = value?.as_str()?;
    if raw.len() > 2048 {
        return None;
    }
    let url = url::Url::parse(raw).ok()?;
    (url.scheme() == "https" && url.host_str().is_some() && url.username().is_empty())
        .then(|| url.to_string())
}

fn version(value: &str) -> Option<String> {
    let v = clean_text(value, 100);
    (!v.is_empty()).then_some(v)
}

/// One compatibility entry: a list of package entries, or `{package: [versions]}`.
fn compatibility(value: &Value) -> Vec<Target> {
    let mut out = Vec::new();
    let mut push = |package: &str, name: Option<String>, versions: Vec<String>| {
        if valid_package(package) && out.len() < 64 {
            out.push(Target {
                package: package.to_owned(),
                name,
                versions,
            });
        }
    };
    let versions_of = |list: Option<&Value>| -> Vec<String> {
        let mut found = Vec::new();
        for entry in list.and_then(Value::as_array).into_iter().flatten() {
            let v = match entry {
                Value::String(s) => version(s),
                Value::Object(o) => o.get("version").and_then(Value::as_str).and_then(version),
                _ => None,
            };
            if let Some(v) = v
                && !found.contains(&v)
                && found.len() < MAX_VERSIONS
            {
                found.push(v);
            }
        }
        found
    };
    match value {
        Value::Array(entries) => {
            for entry in entries {
                match entry {
                    Value::String(package) => push(package, None, Vec::new()),
                    Value::Object(o) => {
                        let Some(package) = o
                            .get("packageName")
                            .or_else(|| o.get("name"))
                            .and_then(Value::as_str)
                        else {
                            continue;
                        };
                        let list = o.get("targets").or_else(|| o.get("versions"));
                        let name = o
                            .get("packageName")
                            .is_some()
                            .then(|| text(o.get("name"), MAX_NAME))
                            .flatten();
                        push(package, name, versions_of(list));
                    }
                    _ => {}
                }
            }
        }
        Value::Object(map) => {
            for (package, list) in map.iter().take(64) {
                push(package, None, versions_of(Some(list)));
            }
        }
        _ => {}
    }
    out
}

/// Parses and validates an index. `excluded` packages (the apps Kiwi Builds builds itself)
/// are left out of the app list.
pub fn parse(bytes: &[u8], excluded: &BTreeSet<String>) -> Result<Index, String> {
    let root: Value = serde_json::from_slice(bytes).map_err(|e| format!("not JSON: {e}"))?;
    let root = root.as_object().ok_or("not a Morphe index")?;
    let bundles = root
        .get("bundles")
        .and_then(Value::as_array)
        .ok_or("no bundle list")?;
    let store = root
        .get("store")
        .and_then(Value::as_object)
        .ok_or("no app list")?;
    let compat = root
        .get("compatibilities")
        .and_then(Value::as_array)
        .ok_or("no compatibility list")?;
    let featured = match root.get("featured") {
        None | Some(Value::Null) => &[][..],
        Some(Value::Array(list)) => &list[..],
        Some(_) => return Err("unreadable featured list".into()),
    };
    if bundles.len() > MAX_BUNDLES
        || store.len() > MAX_STORE
        || compat.len() > MAX_COMPATIBILITIES
        || featured.len() > MAX_FEATURED
    {
        return Err("the index is larger than Kiwi accepts".into());
    }
    let compatibilities: Vec<Vec<Target>> = compat.iter().map(compatibility).collect();
    let mut total = 0usize;
    let mut parsed = Vec::new();
    let mut seen = BTreeSet::new();
    for bundle in bundles {
        let Some(b) = bundle.as_object() else {
            continue;
        };
        let patches = b.get("patches").and_then(Value::as_array);
        total += patches.map_or(0, Vec::len);
        if total > MAX_PATCHES {
            return Err("the index is larger than Kiwi accepts".into());
        }
        if let Some(bundle) = parse_bundle(b, compatibilities.len())
            && seen.insert(bundle.repo.to_ascii_lowercase())
        {
            parsed.push(bundle);
        }
    }
    if parsed.is_empty() {
        return Err("the index lists no usable bundles".into());
    }
    let featured: Vec<String> = {
        let mut out = Vec::new();
        for package in featured.iter().filter_map(Value::as_str) {
            if valid_package(package) && !out.iter().any(|p| p == package) {
                out.push(package.to_owned());
            }
        }
        out
    };
    let mut index = Index {
        bundles: parsed,
        compatibilities,
        apps: BTreeMap::new(),
        featured: Vec::new(),
    };
    index.apps = apps(&index, store, &featured, excluded);
    index.featured = featured
        .into_iter()
        .filter(|p| index.apps.contains_key(p))
        .collect();
    Ok(index)
}

fn parse_bundle(b: &Map<String, Value>, compatibilities: usize) -> Option<Bundle> {
    if b.get("source").and_then(Value::as_str) != Some("github") {
        return None;
    }
    let repo = b.get("repo").and_then(Value::as_str)?;
    if !valid_repo(repo) {
        return None;
    }
    let mut patches = Vec::new();
    for p in b.get("patches")?.as_array()? {
        let Some(p) = p.as_object() else {
            continue;
        };
        let Some(name) = text(p.get("name"), MAX_NAME) else {
            continue;
        };
        let key = p
            .get("compatiblePackagesKey")
            .or_else(|| p.get("compatibilityKey"));
        let compatibility = match key {
            None | Some(Value::Null) => None,
            Some(k) => match k.as_u64().map(|k| k as usize) {
                Some(k) if k < compatibilities => Some(k),
                _ => continue,
            },
        };
        patches.push(Patch {
            name,
            description: text(p.get("description"), MAX_TEXT).unwrap_or_default(),
            default: p.get("default").and_then(Value::as_bool).unwrap_or(true),
            compatibility,
        });
        if patches.len() >= MAX_PATCHES_PER_BUNDLE {
            break;
        }
    }
    let app_updates = b
        .get("appUpdates")
        .and_then(Value::as_object)
        .map(|m| {
            m.iter()
                .filter(|(k, _)| valid_package(k))
                .filter_map(|(k, v)| Some((k.clone(), millis(Some(v))?)))
                .take(1000)
                .collect()
        })
        .unwrap_or_default();
    Some(Bundle {
        repo: repo.to_owned(),
        author: text(b.get("author"), MAX_NAME)
            .unwrap_or_else(|| repo.split('/').next().unwrap_or_default().to_owned()),
        stars: count(b.get("stars")),
        updated_at: millis(b.get("updatedAt")),
        app_updates,
        targets: b
            .get("targetApps")
            .and_then(Value::as_array)
            .map_or(0, Vec::len),
        patches,
    })
}

fn apps(
    index: &Index,
    store: &Map<String, Value>,
    featured: &[String],
    excluded: &BTreeSet<String>,
) -> BTreeMap<String, App> {
    // Per package: bundle -> patch count, and the patch text for search.
    let mut found: HashMap<&str, (BTreeMap<usize, usize>, Vec<&Patch>, Option<&str>)> =
        HashMap::new();
    for (number, bundle) in index.bundles.iter().enumerate() {
        for patch in &bundle.patches {
            let Some(targets) = patch.compatibility.map(|c| &index.compatibilities[c]) else {
                continue;
            };
            let mut counted = BTreeSet::new();
            for target in targets {
                if excluded.contains(&target.package) || !counted.insert(&target.package) {
                    continue;
                }
                let entry = found.entry(&target.package).or_default();
                *entry.0.entry(number).or_default() += 1;
                if entry.1.len() < 2000 {
                    entry.1.push(patch);
                }
                if entry.2.is_none() {
                    entry.2 = target.name.as_deref();
                }
            }
        }
    }
    found
        .into_iter()
        .map(|(package, (bundles, patches, compat_name))| {
            let info = store.get(package).and_then(Value::as_object);
            let field = |key: &str| info.and_then(|i| i.get(key));
            let name = text(field("name"), MAX_NAME)
                .or_else(|| compat_name.map(str::to_owned))
                .unwrap_or_else(|| package.to_owned());
            let mut patch_text = String::new();
            for patch in patches {
                patch_text.push_str(&patch.name.to_lowercase());
                patch_text.push('\n');
                patch_text.push_str(&patch.description.to_lowercase());
                patch_text.push('\n');
            }
            let app = App {
                package: package.to_owned(),
                names: format!("{}\n{}", name.to_lowercase(), package.to_lowercase()),
                name,
                category: text(field("category"), MAX_NAME),
                icon_url: icon(field("iconUrl")),
                description: text(field("description"), MAX_TEXT),
                installs: count(field("installs")),
                featured: featured.iter().any(|f| f == package),
                patches: bundles.values().copied().max().unwrap_or(0),
                bundles: bundles.into_keys().collect(),
                patch_text,
            };
            (package.to_owned(), app)
        })
        .collect()
}

/// Compares two version names part by part: numbers as numbers, other parts as text.
/// "Any" (empty) sorts before every version.
pub fn compare_versions(a: &str, b: &str) -> Ordering {
    let parts = |v: &str| -> Vec<String> {
        v.split(|c: char| matches!(c, '.' | '-' | '_' | ' '))
            .filter(|p| !p.is_empty())
            .map(str::to_owned)
            .collect()
    };
    let (a, b) = (parts(a), parts(b));
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (
            a.get(i).map(String::as_str).unwrap_or(""),
            b.get(i).map(String::as_str).unwrap_or(""),
        );
        let order = match (x.parse::<u64>(), y.parse::<u64>()) {
            (Ok(x), Ok(y)) => x.cmp(&y),
            (Ok(_), Err(_)) if y.is_empty() => Ordering::Greater,
            (Err(_), Ok(_)) if x.is_empty() => Ordering::Less,
            _ => x.cmp(y),
        };
        if order != Ordering::Equal {
            return order;
        }
    }
    Ordering::Equal
}

impl Index {
    /// The bundles with patches for `package`, best first by what the index says: bundles
    /// with patches on by default first, then the newest target version, the most default
    /// patches, the most recent update, the fewest other apps and the most stars. Kiwi
    /// checks the top ones against their actual releases before building.
    pub fn candidates(&self, package: &str) -> Vec<Candidate> {
        let Some(app) = self.apps.get(package) else {
            return Vec::new();
        };
        let mut out: Vec<Candidate> = app
            .bundles
            .iter()
            .map(|&number| {
                let bundle = &self.bundles[number];
                let mut patches = 0;
                let mut default_patches = 0;
                let mut versions: Vec<String> = Vec::new();
                for patch in &bundle.patches {
                    let Some(target) = patch.compatibility.and_then(|c| {
                        self.compatibilities[c]
                            .iter()
                            .find(|t| t.package == package)
                    }) else {
                        continue;
                    };
                    patches += 1;
                    if patch.default {
                        default_patches += 1;
                    }
                    for v in &target.versions {
                        if !versions.contains(v) && versions.len() < MAX_VERSIONS {
                            versions.push(v.clone());
                        }
                    }
                }
                versions.sort_by(|a, b| compare_versions(b, a));
                Candidate {
                    bundle: number,
                    repo: bundle.repo.clone(),
                    author: bundle.author.clone(),
                    stars: bundle.stars,
                    updated_at: bundle
                        .app_updates
                        .get(package)
                        .copied()
                        .or(bundle.updated_at),
                    patches,
                    default_patches,
                    versions,
                    targets: bundle.targets,
                }
            })
            .collect();
        out.sort_by(|a, b| {
            (b.default_patches > 0)
                .cmp(&(a.default_patches > 0))
                .then_with(|| {
                    compare_versions(
                        b.versions.first().map_or("", String::as_str),
                        a.versions.first().map_or("", String::as_str),
                    )
                })
                .then_with(|| b.default_patches.cmp(&a.default_patches))
                .then_with(|| b.updated_at.cmp(&a.updated_at))
                .then_with(|| a.targets.cmp(&b.targets))
                .then_with(|| b.stars.cmp(&a.stars))
                .then_with(|| a.repo.cmp(&b.repo))
        });
        out
    }

    /// Featured apps in index order, then the most installed others, `limit` in all.
    pub fn popular(&self, limit: usize) -> Vec<&App> {
        let mut out: Vec<&App> = self
            .featured
            .iter()
            .filter_map(|p| self.apps.get(p))
            .take(limit)
            .collect();
        if out.len() < limit {
            let mut rest: Vec<&App> = self.apps.values().filter(|a| !a.featured).collect();
            rest.sort_by(|a, b| by_installs(a, b));
            out.extend(rest.into_iter().take(limit - out.len()));
        }
        out
    }

    /// Apps whose name, package or patches mention `query` (case-insensitive), apps whose
    /// name or package matches first, each group by installs. An empty query lists every
    /// app by installs.
    pub fn search(&self, query: &str) -> Vec<&App> {
        let query = query.trim().to_lowercase();
        let mut hits: Vec<(bool, &App)> = self
            .apps
            .values()
            .filter_map(|app| {
                if query.is_empty() || app.names.contains(&query) {
                    Some((true, app))
                } else if app.patch_text.contains(&query) {
                    Some((false, app))
                } else {
                    None
                }
            })
            .collect();
        hits.sort_by(|(a_name, a), (b_name, b)| b_name.cmp(a_name).then_with(|| by_installs(a, b)));
        hits.into_iter().map(|(_, app)| app).collect()
    }
}

fn by_installs(a: &App, b: &App) -> Ordering {
    b.installs
        .cmp(&a.installs)
        .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        .then_with(|| a.package.cmp(&b.package))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    pub fn sample() -> Value {
        json!({
            "bundles": [
                {"source": "github", "repo": "VenusIsJaded/Venus-Patches", "name": "Venus", "author": "VenusIsJaded",
                 "stars": 180, "updatedAt": 1793697356000_i64, "targetApps": ["com.discord"],
                 "patches": [
                    {"name": "Venus settings", "description": "Adds Venus to Discord's settings", "default": true, "compatiblePackagesKey": 0},
                    {"name": "Custom branding", "description": "Installs Venus next to Discord", "default": false, "compatiblePackagesKey": 0},
                    {"name": "Universal thing", "description": "Any app"}
                 ]},
                {"source": "github", "repo": "lildanlid1/Cordly", "author": "lildanlid1", "stars": 3,
                 "updatedAt": 1793797356000_i64, "targetApps": ["com.discord"],
                 "patches": [{"name": "Cordly settings", "compatiblePackagesKey": 0, "default": true}]},
                {"source": "gitlab", "repo": "early.egg3707/ee-morphe-patches", "patches": [{"name": "x", "compatiblePackagesKey": 1}]},
                {"source": "github", "repo": "../../etc", "patches": [{"name": "x", "compatiblePackagesKey": 1}]},
                {"source": "github", "repo": "someone/old-discord", "author": "someone", "stars": 900,
                 "updatedAt": 1700000000000_i64, "targetApps": ["com.discord", "com.pinterest"],
                 "patches": [{"name": "Old", "compatiblePackagesKey": 2, "default": true},
                             {"name": "Pins ad blocker", "description": "Blocks promoted pins", "compatiblePackagesKey": 1}]},
                {"source": "github", "repo": "kiwi/reddit", "patches": [{"name": "Reddit thing", "compatiblePackagesKey": 3}]}
            ],
            "store": {
                "com.discord": {"name": "Discord", "category": "Social &amp; Communication", "installs": 701967995,
                                "iconUrl": "https://play-lh.googleusercontent.com/discord=s128", "description": "Group Chat That’s Fun &amp; Games"},
                "com.pinterest": {"name": "Pinterest\u{202e}", "installs": 1000, "iconUrl": "http://insecure.example/icon.png"},
                "com.reddit.frontpage": {"name": "Reddit", "installs": 5}
            },
            "compatibilities": [
                [{"packageName": "com.discord", "name": "Discord", "targets": [{"version": "351.11 - Stable", "versionCodes": [351011]}]}],
                [{"packageName": "com.pinterest", "targets": [{"version": "14.43.0"}]}],
                {"com.discord": ["300.1", "299.0"]},
                [{"packageName": "com.reddit.frontpage", "targets": []}]
            ],
            "featured": ["com.discord", "com.discord", "not a package", "com.reddit.frontpage", "com.unknown.app"]
        })
    }

    fn parsed() -> Index {
        parse(
            sample().to_string().as_bytes(),
            &BTreeSet::from(["com.reddit.frontpage".to_owned()]),
        )
        .unwrap()
    }

    #[test]
    fn validates_names_and_repositories() {
        assert!(valid_package("com.discord"));
        assert!(valid_package("tv.twitch.android.app"));
        assert!(valid_package("a.b_c.D9"));
        for bad in [
            "discord", "1com.x", "com..x", "com.x.", ".com.x", "com.x-y", "com/x", "",
        ] {
            assert!(!valid_package(bad), "{bad}");
        }
        assert!(valid_repo("VenusIsJaded/Venus-Patches"));
        assert!(valid_repo("a/b.c_d"));
        for bad in [
            "early.egg3707/x",
            "../../etc",
            "a/..",
            "a/.",
            "-a/b",
            "a--b/c",
            "a/b/c",
            "a",
            "a/",
            "a/b c",
            "a/b%2F",
        ] {
            assert!(!valid_repo(bad), "{bad}");
        }
    }

    #[test]
    fn cleans_text_from_the_index() {
        assert_eq!(
            clean_text("Fun &amp; Games\u{202e}\n\tagain  ", 100),
            "Fun & Games again"
        );
        assert_eq!(clean_text("abcdef", 3), "abc");
        assert_eq!(clean_text("\u{0}\u{7}", 10), "");
    }

    #[test]
    fn parses_apps_bundles_and_featured() {
        let index = parsed();
        assert_eq!(
            index
                .bundles
                .iter()
                .map(|b| b.repo.as_str())
                .collect::<Vec<_>>(),
            [
                "VenusIsJaded/Venus-Patches",
                "lildanlid1/Cordly",
                "someone/old-discord",
                "kiwi/reddit"
            ],
            "GitLab and invalid repositories are skipped"
        );
        assert_eq!(index.featured, ["com.discord"]);
        let discord = &index.apps["com.discord"];
        assert_eq!(discord.name, "Discord");
        assert_eq!(discord.category.as_deref(), Some("Social & Communication"));
        assert_eq!(
            discord.description.as_deref(),
            Some("Group Chat That’s Fun & Games")
        );
        assert_eq!(
            discord.icon_url.as_deref(),
            Some("https://play-lh.googleusercontent.com/discord=s128")
        );
        assert_eq!(discord.bundles.len(), 3);
        assert_eq!(discord.patches, 2);
        assert!(discord.featured);
        let pinterest = &index.apps["com.pinterest"];
        assert_eq!(pinterest.name, "Pinterest", "bidi overrides are dropped");
        assert_eq!(pinterest.icon_url, None, "only https icons");
        assert!(
            !index.apps.contains_key("com.reddit.frontpage"),
            "apps Kiwi Builds builds itself are left out"
        );
    }

    #[test]
    fn ranks_candidates_by_index_facts() {
        let index = parsed();
        let ranked = index.candidates("com.discord");
        let repos: Vec<&str> = ranked.iter().map(|c| c.repo.as_str()).collect();
        assert_eq!(
            repos,
            [
                "lildanlid1/Cordly",
                "VenusIsJaded/Venus-Patches",
                "someone/old-discord"
            ],
            "the same version and default patches: the more recent update first"
        );
        assert_eq!(ranked[1].patches, 2);
        assert_eq!(ranked[1].default_patches, 1);
        assert_eq!(ranked[1].versions, ["351.11 - Stable"]);
        assert_eq!(ranked[2].versions, ["300.1", "299.0"]);
        let mut tied = sample();
        tied["bundles"][1]["updatedAt"] = json!(1600000000000_i64);
        let index = parse(tied.to_string().as_bytes(), &BTreeSet::new()).unwrap();
        assert_eq!(
            index.candidates("com.discord")[0].repo,
            "VenusIsJaded/Venus-Patches"
        );
        assert!(index.candidates("com.nothing").is_empty());
    }

    #[test]
    fn compares_versions() {
        use Ordering::*;
        assert_eq!(compare_versions("351.11 - Stable", "351.9"), Greater);
        assert_eq!(compare_versions("14.43.0", "14.43.0"), Equal);
        assert_eq!(compare_versions("12.40.1-prod.01", "12.41.0-prod.01"), Less);
        assert_eq!(compare_versions("1.2", "1.2.1"), Less);
        assert_eq!(compare_versions("", "0.1"), Less);
        assert_eq!(compare_versions("4.10.18-googleplay", "4.10.9"), Greater);
    }

    #[test]
    fn searches_names_packages_and_patches() {
        let index = parsed();
        let names = |q: &str| -> Vec<String> {
            index.search(q).iter().map(|a| a.package.clone()).collect()
        };
        assert_eq!(names(""), ["com.discord", "com.pinterest"]);
        assert_eq!(names("DISCORD"), ["com.discord"]);
        assert_eq!(names("com.pin"), ["com.pinterest"]);
        assert_eq!(names("promoted pins"), ["com.pinterest"]);
        assert_eq!(names("venus"), ["com.discord"]);
        assert!(names("nothing like this").is_empty());
        let popular: Vec<&str> = index
            .popular(30)
            .iter()
            .map(|a| a.package.as_str())
            .collect();
        assert_eq!(popular, ["com.discord", "com.pinterest"]);
        assert_eq!(index.popular(1).len(), 1);
    }

    #[test]
    fn refuses_hostile_or_broken_indexes() {
        let none = BTreeSet::new();
        for bad in [
            &b"not json"[..],
            b"[]",
            b"{}",
            br#"{"bundles": {}, "store": {}, "compatibilities": []}"#,
            br#"{"bundles": [], "store": {}, "compatibilities": []}"#,
            br#"{"bundles": [{"source": "github", "repo": "a/b", "patches": []}], "store": [], "compatibilities": []}"#,
            br#"{"bundles": [{"source": "github", "repo": "a/b", "patches": []}], "store": {}, "compatibilities": [], "featured": "x"}"#,
        ] {
            assert!(parse(bad, &none).is_err(), "{}", String::from_utf8_lossy(bad));
        }
        // Too many bundles.
        let many: Vec<Value> = (0..MAX_BUNDLES + 1)
            .map(|i| json!({"source": "github", "repo": format!("a/b{i}"), "patches": []}))
            .collect();
        let big = json!({"bundles": many, "store": {}, "compatibilities": []});
        assert!(parse(big.to_string().as_bytes(), &none).is_err());
        // Deep nesting is refused by the JSON reader, not a stack overflow.
        let deep = format!(
            "{{\"bundles\": {}{}, \"store\": {{}}, \"compatibilities\": []}}",
            "[".repeat(10_000),
            "]".repeat(10_000)
        );
        assert!(parse(deep.as_bytes(), &none).is_err());
        // Wrong types and out-of-range keys are skipped, not trusted.
        let odd = json!({
            "bundles": [
                {"source": "github", "repo": "a/b", "stars": -5, "updatedAt": "yesterday",
                 "patches": [
                    {"name": "ok", "compatiblePackagesKey": 0},
                    {"name": "bad key", "compatiblePackagesKey": 99},
                    {"name": "negative key", "compatiblePackagesKey": -1},
                    {"name": 5, "compatiblePackagesKey": 0},
                    "not a patch"
                 ]},
                "not a bundle", null
            ],
            "store": {"com.x.y": {"name": ["x"], "installs": "many", "iconUrl": "javascript:alert(1)"}},
            "compatibilities": [[{"packageName": "com.x.y", "targets": [{"version": {"nested": true}}, "1.0", 7]}, {"packageName": "bad pkg"}]]
        });
        let index = parse(odd.to_string().as_bytes(), &none).unwrap();
        assert_eq!(index.bundles[0].stars, 0);
        assert_eq!(index.bundles[0].updated_at, None);
        assert_eq!(index.bundles[0].patches.len(), 1);
        let app = &index.apps["com.x.y"];
        assert_eq!(app.name, "com.x.y");
        assert_eq!(app.installs, 0);
        assert_eq!(app.icon_url, None);
        assert_eq!(index.candidates("com.x.y")[0].versions, ["1.0"]);
        assert_eq!(index.compatibilities[0].len(), 1);
    }
}
