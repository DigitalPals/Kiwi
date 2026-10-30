//! Builds the entitlements a bundle is signed with from its profile.

use plist::{Dictionary, Value};

use crate::Profile;

/// Entitlements Kiwi always sets from the profile, whatever the binary requested.
const MANAGED: [&str; 4] = [
    "application-identifier",
    "com.apple.developer.team-identifier",
    "get-task-allow",
    "keychain-access-groups",
];

/// Entitlements for `bundle_id` signed with `profile`, plus the keys `original`
/// requested that the profile cannot grant (sorted).
pub(crate) fn build(
    profile: &Profile,
    bundle_id: &str,
    original: &Dictionary,
) -> (Dictionary, Vec<String>) {
    let app_id = format!("{}.{bundle_id}", profile.app_id_prefix());
    let mut ent = Dictionary::new();
    ent.insert("application-identifier".into(), app_id.clone().into());
    ent.insert(
        "com.apple.developer.team-identifier".into(),
        profile.team_id.clone().into(),
    );
    ent.insert("get-task-allow".into(), profile.development.into());
    ent.insert(
        "keychain-access-groups".into(),
        Value::Array(vec![app_id.into()]),
    );
    let mut dropped = Vec::new();
    for (key, requested) in original {
        if MANAGED.contains(&key.as_str()) {
            continue;
        }
        match profile
            .entitlements
            .get(key)
            .and_then(|allowed| grant(allowed, requested))
        {
            Some(value) => {
                ent.insert(key.clone(), value);
            }
            None => dropped.push(key.clone()),
        }
    }
    dropped.sort();
    (ent, dropped)
}

/// The value to sign with when the binary requests `requested` and the profile allows
/// `allowed`: the requested value where the profile's patterns permit it, otherwise the
/// profile's own concrete values (e.g. its app groups or `aps-environment`).
fn grant(allowed: &Value, requested: &Value) -> Option<Value> {
    if allowed == requested {
        return Some(requested.clone());
    }
    if let Value::Boolean(allowed) = allowed {
        return (*allowed && matches!(requested, Value::Boolean(_))).then(|| requested.clone());
    }
    let patterns: Vec<&str> = match allowed {
        Value::String(s) => vec![s],
        Value::Array(items) => items.iter().filter_map(Value::as_string).collect(),
        _ => return None,
    };
    let permitted = |v: &Value| {
        v.as_string()
            .is_some_and(|s| patterns.iter().any(|p| glob(p, s)))
    };
    let mut concrete = patterns.iter().filter(|p| !p.contains('*'));
    match requested {
        Value::String(_) if permitted(requested) => Some(requested.clone()),
        Value::String(_) => concrete.next().map(|p| Value::from(*p)),
        Value::Array(items) => {
            let mut kept: Vec<Value> = items.iter().filter(|v| permitted(v)).cloned().collect();
            if kept.is_empty() {
                kept = concrete.map(|p| Value::from(*p)).collect();
            }
            (!kept.is_empty()).then_some(Value::Array(kept))
        }
        _ => None,
    }
}

/// Entitlement patterns only use a trailing `*`.
fn glob(pattern: &str, value: &str) -> bool {
    match pattern.strip_suffix('*') {
        Some(prefix) => value.starts_with(prefix),
        None => pattern == value,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{ProfileSpec, TEAM, profile};

    fn strings(items: &[&str]) -> Value {
        Value::Array(items.iter().map(|s| (*s).into()).collect())
    }

    fn dict(entries: &[(&str, Value)]) -> Dictionary {
        entries
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect()
    }

    #[test]
    fn wildcard_profile_grants_basics_only() {
        let p = profile(&ProfileSpec::wildcard());
        let original = dict(&[
            (
                "application-identifier",
                "OLDTEAM.com.google.ios.youtube".into(),
            ),
            (
                "keychain-access-groups",
                strings(&["OLDTEAM.com.google.common.SSO"]),
            ),
            (
                "com.apple.security.application-groups",
                strings(&["group.com.google"]),
            ),
            ("aps-environment", "production".into()),
        ]);
        let (ent, dropped) = build(&p, "com.google.ios.youtube.TEAM123456", &original);
        let app_id = format!("{TEAM}.com.google.ios.youtube.TEAM123456");
        assert_eq!(
            ent,
            dict(&[
                ("application-identifier", app_id.clone().into()),
                ("com.apple.developer.team-identifier", TEAM.into()),
                ("get-task-allow", true.into()),
                ("keychain-access-groups", strings(&[&app_id])),
            ])
        );
        assert_eq!(
            dropped,
            vec!["aps-environment", "com.apple.security.application-groups"]
        );
    }

    #[test]
    fn explicit_profile_grants_requested_capabilities() {
        let mut spec = ProfileSpec::explicit("com.example.app");
        spec.development = false;
        spec.entitlements = vec![
            (
                "com.apple.security.application-groups",
                strings(&["group.com.example.kiwi"]),
            ),
            ("aps-environment", "development".into()),
            ("com.apple.developer.associated-domains", "*".into()),
            (
                "com.apple.developer.icloud-container-identifiers",
                strings(&["iCloud.com.example.*"]),
            ),
            ("com.apple.developer.healthkit", true.into()),
            ("com.apple.developer.siri", false.into()),
        ];
        let p = profile(&spec);
        let original = dict(&[
            (
                "com.apple.security.application-groups",
                strings(&["group.com.vendor"]),
            ),
            ("aps-environment", "production".into()),
            (
                "com.apple.developer.associated-domains",
                strings(&["applinks:example.com"]),
            ),
            (
                "com.apple.developer.icloud-container-identifiers",
                strings(&["iCloud.com.example.a", "iCloud.other"]),
            ),
            ("com.apple.developer.healthkit", true.into()),
            ("com.apple.developer.siri", true.into()),
            ("com.apple.private.security.no-sandbox", true.into()),
        ]);
        let (ent, dropped) = build(&p, "com.example.app", &original);
        assert_eq!(ent.get("get-task-allow"), Some(&false.into()));
        assert_eq!(
            ent.get("com.apple.security.application-groups"),
            Some(&strings(&["group.com.example.kiwi"]))
        );
        assert_eq!(ent.get("aps-environment"), Some(&"development".into()));
        assert_eq!(
            ent.get("com.apple.developer.associated-domains"),
            Some(&strings(&["applinks:example.com"]))
        );
        assert_eq!(
            ent.get("com.apple.developer.icloud-container-identifiers"),
            Some(&strings(&["iCloud.com.example.a"]))
        );
        assert_eq!(ent.get("com.apple.developer.healthkit"), Some(&true.into()));
        assert_eq!(
            dropped,
            vec![
                "com.apple.developer.siri",
                "com.apple.private.security.no-sandbox"
            ]
        );
    }

    #[test]
    fn grant_rules() {
        assert_eq!(grant(&"TEAM.*".into(), &"OTHER.x".into()), None);
        assert_eq!(
            grant(&"TEAM.*".into(), &"TEAM.x".into()),
            Some("TEAM.x".into())
        );
        assert_eq!(grant(&strings(&[]), &strings(&["a"])), None);
        assert_eq!(grant(&1.into(), &2.into()), None);
        assert_eq!(grant(&1.into(), &1.into()), Some(1.into()));
        assert_eq!(grant(&true.into(), &"yes".into()), None);
        assert!(glob("*", "anything"));
        assert!(!glob("a.b", "a.bc"));
    }
}
