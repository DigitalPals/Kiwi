//! Nested code discovery and Info.plist edits (bundle ids, URL scheme, cleanup).

use std::{
    collections::HashSet,
    fs,
    io::{self, Read},
    os::unix::fs::PermissionsExt,
    path::{Path, PathBuf},
};

use plist::{Dictionary, Value};

use crate::{Error, MAX_PLIST_BYTES, Result, check_plist};

/// `CFBundleURLName` of the URL type Kiwi injects.
const URL_NAME: &str = "kiwi";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum CodeKind {
    Dylib,
    Framework,
    /// `.app` or `.appex`: gets a profile and entitlements.
    Bundle,
}

#[derive(Debug)]
pub(crate) struct Code {
    pub path: PathBuf,
    pub kind: CodeKind,
}

#[derive(Debug)]
pub(crate) struct BundleInfo {
    pub path: PathBuf,
    pub old_id: String,
    pub new_id: String,
    pub executable: Option<PathBuf>,
}

#[derive(Debug)]
pub(crate) struct Prepared {
    /// The main app first, then nested apps and extensions.
    pub bundles: Vec<BundleInfo>,
    /// Nested code in signing order (deepest first); excludes the main app.
    pub code: Vec<Code>,
    pub version: String,
    pub build: Option<String>,
}

/// `Payload/…` part of a path inside the extracted IPA, for messages.
pub(crate) fn short(path: &Path) -> String {
    let s = path.to_string_lossy();
    s.find("/Payload/")
        .map_or_else(|| s.to_string(), |i| s[i + 1..].to_owned())
}

struct InfoPlist {
    path: PathBuf,
    dict: Dictionary,
    binary: bool,
}

impl InfoPlist {
    fn load(bundle: &Path) -> Result<Self> {
        let path = bundle.join("Info.plist");
        let mut bytes = Vec::new();
        fs::File::open(&path)
            .and_then(|f| f.take(MAX_PLIST_BYTES + 1).read_to_end(&mut bytes))
            .map_err(|e| Error::io(format!("Could not read {}", short(&path)), e))?;
        if bytes.len() as u64 > MAX_PLIST_BYTES {
            return Err(Error::InvalidIpa(format!(
                "{} is larger than {} MiB",
                short(&path),
                MAX_PLIST_BYTES >> 20
            )));
        }
        let invalid =
            |e: String| Error::InvalidIpa(format!("{} is not a valid plist: {e}", short(&path)));
        check_plist(&bytes).map_err(invalid)?;
        let dict = plist::from_bytes(&bytes).map_err(|e| invalid(e.to_string()))?;
        Ok(Self {
            binary: bytes.starts_with(b"bplist"),
            path,
            dict,
        })
    }

    fn save(&self) -> Result<()> {
        // Replace rather than write through: the old file may be a hard or symbolic link.
        remove_path(&self.path)?;
        let result = if self.binary {
            plist::to_file_binary(&self.path, &self.dict)
        } else {
            plist::to_file_xml(&self.path, &self.dict)
        };
        result.map_err(|e| Error::InvalidIpa(format!("Could not write {}: {e}", short(&self.path))))
    }

    fn string(&self, key: &str) -> Option<&str> {
        self.dict.get(key).and_then(Value::as_string)
    }

    fn bundle_id(&self) -> Result<String> {
        self.string("CFBundleIdentifier")
            .map(str::to_owned)
            .ok_or_else(|| {
                Error::InvalidIpa(format!("{} has no CFBundleIdentifier", short(&self.path)))
            })
    }
}

/// Rewrites bundle ids, injects the URL scheme and removes what must not be re-signed.
pub(crate) fn prepare(
    app: &Path,
    bundle_id: &str,
    url_scheme: Option<&str>,
    strip_watch: bool,
) -> Result<Prepared> {
    if strip_watch {
        remove_path(&app.join("Watch"))?;
        remove_path(&app.join("com.apple.WatchPlaceholder"))?;
    }
    let mut info = InfoPlist::load(app)?;
    let old_main = info.bundle_id()?;
    let version = info
        .string("CFBundleShortVersionString")
        .or_else(|| info.string("CFBundleVersion"))
        .unwrap_or_default()
        .to_owned();
    let build = info.string("CFBundleVersion").map(str::to_owned);
    info.dict
        .insert("CFBundleIdentifier".into(), bundle_id.into());
    if let Some(scheme) = url_scheme {
        add_url_scheme(&mut info.dict, scheme);
    }
    let mut bundles = vec![finish(app, info, old_main.clone(), bundle_id.to_owned())?];

    let code = nested_code(app)?;
    for item in code.iter().filter(|c| c.kind == CodeKind::Bundle) {
        let mut info = InfoPlist::load(&item.path)?;
        let old_id = info.bundle_id()?;
        let new_id = map_bundle_id(&old_main, bundle_id, &old_id);
        info.dict
            .insert("CFBundleIdentifier".into(), new_id.clone().into());
        if let Some(Value::String(id)) = info.dict.get_mut("WKCompanionAppBundleIdentifier") {
            *id = map_bundle_id(&old_main, bundle_id, id);
        }
        if let Some(Value::String(id)) = info
            .dict
            .get_mut("NSExtension")
            .and_then(Value::as_dictionary_mut)
            .and_then(|e| e.get_mut("NSExtensionAttributes"))
            .and_then(Value::as_dictionary_mut)
            .and_then(|a| a.get_mut("WKAppBundleIdentifier"))
        {
            *id = map_bundle_id(&old_main, bundle_id, id);
        }
        bundles.push(finish(&item.path, info, old_id, new_id)?);
    }

    let mut seen = HashSet::new();
    if let Some(dup) = bundles.iter().find(|b| !seen.insert(&b.new_id)) {
        return Err(Error::InvalidIpa(format!(
            "Two bundles in the app would both be signed as {}",
            dup.new_id
        )));
    }
    Ok(Prepared {
        bundles,
        code,
        version,
        build,
    })
}

fn finish(path: &Path, mut info: InfoPlist, old_id: String, new_id: String) -> Result<BundleInfo> {
    info.dict.remove("UISupportedDevices");
    info.save()?;
    remove_path(&path.join("SC_Info"))?;
    let executable = info
        .string("CFBundleExecutable")
        .filter(|name| !name.is_empty() && !name.contains('/') && *name != "..")
        .map(|name| path.join(name));
    Ok(BundleInfo {
        path: path.to_owned(),
        old_id,
        new_id,
        executable,
    })
}

/// New id of a nested bundle: `OLD.suffix` becomes `NEW.suffix`, anything else
/// `NEW.<last component>`.
pub(crate) fn map_bundle_id(old_main: &str, new_main: &str, id: &str) -> String {
    if id == old_main {
        return new_main.to_owned();
    }
    match id
        .strip_prefix(old_main)
        .and_then(|rest| rest.strip_prefix('.'))
    {
        Some(suffix) => format!("{new_main}.{suffix}"),
        None => format!("{new_main}.{}", id.rsplit('.').next().unwrap_or(id)),
    }
}

/// Adds (or replaces) Kiwi's entry in `CFBundleURLTypes`, keeping the others.
pub(crate) fn add_url_scheme(info: &mut Dictionary, scheme: &str) {
    let mut types = match info.remove("CFBundleURLTypes") {
        Some(Value::Array(types)) => types,
        _ => Vec::new(),
    };
    types.retain(|t| {
        t.as_dictionary()
            .and_then(|d| d.get("CFBundleURLName"))
            .and_then(Value::as_string)
            != Some(URL_NAME)
    });
    let mut entry = Dictionary::new();
    entry.insert("CFBundleURLName".into(), URL_NAME.into());
    entry.insert(
        "CFBundleURLSchemes".into(),
        Value::Array(vec![scheme.into()]),
    );
    types.push(entry.into());
    info.insert("CFBundleURLTypes".into(), Value::Array(types));
}

/// Nested code below `app` in signing order: deepest path first.
fn nested_code(app: &Path) -> Result<Vec<Code>> {
    let mut found = Vec::new();
    walk(app, &mut found).map_err(|e| Error::io(format!("Could not read {}", short(app)), e))?;
    found.sort_by(|a, b| {
        let depth = |c: &Code| c.path.components().count();
        depth(b).cmp(&depth(a)).then_with(|| a.path.cmp(&b.path))
    });
    Ok(found)
}

fn walk(dir: &Path, found: &mut Vec<Code>) -> io::Result<()> {
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let file_type = entry.file_type()?; // symlinks are not followed
        let ext = path.extension().and_then(|e| e.to_str());
        let kind = if file_type.is_dir() {
            walk(&path, found)?;
            match ext {
                Some("app" | "appex") => CodeKind::Bundle,
                Some("framework") => CodeKind::Framework,
                _ => continue,
            }
        } else if file_type.is_file() && ext == Some("dylib") {
            CodeKind::Dylib
        } else {
            continue;
        };
        found.push(Code { path, kind });
    }
    Ok(())
}

/// Sets directories and executables to 0755 and other files to 0644, whatever the
/// umask was when they were extracted or signed: installed apps are owned by another
/// user than the one they run as.
pub(crate) fn normalize_permissions(dir: &Path) -> Result<()> {
    fn walk(dir: &Path) -> io::Result<()> {
        fs::set_permissions(dir, fs::Permissions::from_mode(0o755))?;
        for entry in fs::read_dir(dir)? {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                walk(&entry.path())?;
            } else if file_type.is_file() {
                let mode = entry.metadata()?.permissions().mode();
                let mode = if mode & 0o100 != 0 { 0o755 } else { 0o644 };
                fs::set_permissions(entry.path(), fs::Permissions::from_mode(mode))?;
            }
        }
        Ok(())
    }
    walk(dir).map_err(|e| Error::io(format!("Could not set permissions in {}", short(dir)), e))
}

/// Removes a file, symlink or directory tree; missing paths are fine.
pub(crate) fn remove_path(path: &Path) -> Result<()> {
    let result = match fs::symlink_metadata(path) {
        Ok(meta) if meta.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e),
    };
    result.map_err(|e| Error::io(format!("Could not remove {}", short(path)), e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::plist_bytes;

    fn dict(entries: &[(&str, Value)]) -> Dictionary {
        entries
            .iter()
            .map(|(k, v)| ((*k).to_owned(), v.clone()))
            .collect()
    }

    fn write_bundle(dir: &Path, entries: &[(&str, Value)], binary: bool) {
        fs::create_dir_all(dir).unwrap();
        fs::write(dir.join("Info.plist"), plist_bytes(dict(entries), binary)).unwrap();
    }

    fn read_info(dir: &Path) -> (Dictionary, bool) {
        let bytes = fs::read(dir.join("Info.plist")).unwrap();
        (
            plist::from_bytes(&bytes).unwrap(),
            bytes.starts_with(b"bplist"),
        )
    }

    fn get<'a>(d: &'a Dictionary, key: &str) -> Option<&'a str> {
        d.get(key).and_then(Value::as_string)
    }

    #[test]
    fn maps_nested_bundle_ids() {
        let map = |id| map_bundle_id("com.google.ios.youtube", "com.google.ios.youtube.T", id);
        assert_eq!(map("com.google.ios.youtube"), "com.google.ios.youtube.T");
        assert_eq!(
            map("com.google.ios.youtube.OpenYouTube.Extension"),
            "com.google.ios.youtube.T.OpenYouTube.Extension"
        );
        assert_eq!(
            map("com.google.ios.youtubeX.widget"),
            "com.google.ios.youtube.T.widget"
        );
        assert_eq!(map("org.vendor.share"), "com.google.ios.youtube.T.share");
        assert_eq!(map("plain"), "com.google.ios.youtube.T.plain");
    }

    #[test]
    fn injects_url_scheme() {
        let mut info = Dictionary::new();
        add_url_scheme(&mut info, "kiwi-abc");
        let expected = Value::Array(vec![
            dict(&[
                ("CFBundleURLName", "kiwi".into()),
                ("CFBundleURLSchemes", Value::Array(vec!["kiwi-abc".into()])),
            ])
            .into(),
        ]);
        assert_eq!(info.get("CFBundleURLTypes"), Some(&expected));

        let existing: Value =
            dict(&[("CFBundleURLSchemes", Value::Array(vec!["youtube".into()]))]).into();
        let mut info = dict(&[("CFBundleURLTypes", Value::Array(vec![existing.clone()]))]);
        add_url_scheme(&mut info, "kiwi-old");
        add_url_scheme(&mut info, "kiwi-new");
        let types = info
            .get("CFBundleURLTypes")
            .and_then(Value::as_array)
            .unwrap();
        assert_eq!(types.len(), 2);
        assert_eq!(types[0], existing);
        assert_eq!(
            types[1]
                .as_dictionary()
                .and_then(|d| d.get("CFBundleURLSchemes")),
            Some(&Value::Array(vec!["kiwi-new".into()]))
        );

        let mut info = dict(&[("CFBundleURLTypes", "broken".into())]);
        add_url_scheme(&mut info, "kiwi-x");
        assert_eq!(
            info.get("CFBundleURLTypes")
                .and_then(Value::as_array)
                .map(Vec::len),
            Some(1)
        );
    }

    fn sample_app(root: &Path) -> PathBuf {
        let app = root.join("Payload/Demo.app");
        write_bundle(
            &app,
            &[
                ("CFBundleIdentifier", "com.example.demo".into()),
                ("CFBundleExecutable", "Demo".into()),
                ("CFBundleShortVersionString", "1.2".into()),
                ("CFBundleVersion", "42".into()),
                (
                    "UISupportedDevices",
                    Value::Array(vec!["iPhone12,1".into()]),
                ),
            ],
            true,
        );
        fs::create_dir_all(app.join("SC_Info")).unwrap();
        fs::write(app.join("SC_Info/Demo.sinf"), b"drm").unwrap();
        fs::create_dir_all(app.join("Frameworks/Kit.framework")).unwrap();
        fs::write(app.join("Frameworks/Lib.dylib"), b"").unwrap();
        fs::write(app.join("Frameworks/notes.txt"), b"").unwrap();
        write_bundle(
            &app.join("PlugIns/Share.appex"),
            &[("CFBundleIdentifier", "com.example.demo.Share".into())],
            false,
        );
        write_bundle(
            &app.join("PlugIns/Widget.appex"),
            &[("CFBundleIdentifier", "org.vendor.widget".into())],
            false,
        );
        fs::create_dir_all(app.join("PlugIns/Share.appex/Frameworks/Inner.framework")).unwrap();
        let watch = app.join("Watch/Demo Watch.app");
        write_bundle(
            &watch,
            &[
                ("CFBundleIdentifier", "com.example.demo.watchkitapp".into()),
                ("WKCompanionAppBundleIdentifier", "com.example.demo".into()),
            ],
            false,
        );
        let attributes = dict(&[(
            "WKAppBundleIdentifier",
            "com.example.demo.watchkitapp".into(),
        )]);
        write_bundle(
            &watch.join("PlugIns/Ext.appex"),
            &[
                (
                    "CFBundleIdentifier",
                    "com.example.demo.watchkitapp.watchkitextension".into(),
                ),
                (
                    "NSExtension",
                    dict(&[("NSExtensionAttributes", attributes.into())]).into(),
                ),
            ],
            false,
        );
        app
    }

    #[test]
    fn prepares_app_without_watch() {
        let dir = tempfile::tempdir().unwrap();
        let app = sample_app(dir.path());
        let prepared = prepare(&app, "com.example.demo.T", Some("kiwi-123"), true).unwrap();

        assert!(!app.join("Watch").exists());
        assert!(!app.join("SC_Info").exists());
        assert_eq!(prepared.version, "1.2");
        assert_eq!(prepared.build.as_deref(), Some("42"));
        let (info, binary) = read_info(&app);
        assert!(binary, "binary Info.plist stays binary");
        assert_eq!(get(&info, "CFBundleIdentifier"), Some("com.example.demo.T"));
        assert!(!info.contains_key("UISupportedDevices"));
        assert!(info.contains_key("CFBundleURLTypes"));
        let (share, binary) = read_info(&app.join("PlugIns/Share.appex"));
        assert!(!binary);
        assert_eq!(
            get(&share, "CFBundleIdentifier"),
            Some("com.example.demo.T.Share")
        );

        let ids: Vec<_> = prepared
            .bundles
            .iter()
            .map(|b| (b.old_id.as_str(), b.new_id.as_str()))
            .collect();
        assert_eq!(
            ids,
            vec![
                ("com.example.demo", "com.example.demo.T"),
                ("com.example.demo.Share", "com.example.demo.T.Share"),
                ("org.vendor.widget", "com.example.demo.T.widget"),
            ]
        );
        assert_eq!(prepared.bundles[0].executable, Some(app.join("Demo")));
        assert_eq!(prepared.bundles[1].executable, None);

        let order: Vec<_> = prepared
            .code
            .iter()
            .map(|c| (c.path.strip_prefix(&app).unwrap().to_str().unwrap(), c.kind))
            .collect();
        assert_eq!(
            order,
            vec![
                (
                    "PlugIns/Share.appex/Frameworks/Inner.framework",
                    CodeKind::Framework
                ),
                ("Frameworks/Kit.framework", CodeKind::Framework),
                ("Frameworks/Lib.dylib", CodeKind::Dylib),
                ("PlugIns/Share.appex", CodeKind::Bundle),
                ("PlugIns/Widget.appex", CodeKind::Bundle),
            ]
        );
    }

    #[test]
    fn prepares_watch_app_ids() {
        let dir = tempfile::tempdir().unwrap();
        let app = sample_app(dir.path());
        let prepared = prepare(&app, "com.example.demo.T", None, false).unwrap();
        assert_eq!(prepared.bundles.len(), 5);
        let (info, _) = read_info(&app);
        assert!(!info.contains_key("CFBundleURLTypes"));
        let watch = app.join("Watch/Demo Watch.app");
        let (info, _) = read_info(&watch);
        assert_eq!(
            get(&info, "CFBundleIdentifier"),
            Some("com.example.demo.T.watchkitapp")
        );
        assert_eq!(
            get(&info, "WKCompanionAppBundleIdentifier"),
            Some("com.example.demo.T")
        );
        let (ext, _) = read_info(&watch.join("PlugIns/Ext.appex"));
        assert_eq!(
            get(&ext, "CFBundleIdentifier"),
            Some("com.example.demo.T.watchkitapp.watchkitextension")
        );
        let attrs = ext
            .get("NSExtension")
            .and_then(Value::as_dictionary)
            .and_then(|e| e.get("NSExtensionAttributes"))
            .and_then(Value::as_dictionary)
            .unwrap();
        assert_eq!(
            get(attrs, "WKAppBundleIdentifier"),
            Some("com.example.demo.T.watchkitapp")
        );
        // The watch extension is deeper than the watch app, so it is signed first.
        let bundles: Vec<_> = prepared
            .code
            .iter()
            .filter(|c| c.kind == CodeKind::Bundle)
            .collect();
        assert!(
            bundles[0]
                .path
                .ends_with("Watch/Demo Watch.app/PlugIns/Ext.appex")
        );
    }

    #[test]
    fn normalizes_permissions() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("Payload/A.app");
        fs::create_dir_all(app.join("Sub")).unwrap();
        fs::write(app.join("A"), b"").unwrap();
        fs::write(app.join("Sub/data"), b"").unwrap();
        fs::set_permissions(app.join("A"), fs::Permissions::from_mode(0o700)).unwrap();
        fs::set_permissions(app.join("Sub/data"), fs::Permissions::from_mode(0o600)).unwrap();
        fs::set_permissions(app.join("Sub"), fs::Permissions::from_mode(0o700)).unwrap();
        std::os::unix::fs::symlink("Sub/data", app.join("link")).unwrap();
        normalize_permissions(&dir.path().join("Payload")).unwrap();
        let mode = |p: &str| fs::metadata(app.join(p)).unwrap().permissions().mode() & 0o7777;
        assert_eq!(
            (mode(""), mode("A"), mode("Sub"), mode("Sub/data")),
            (0o755, 0o755, 0o755, 0o644)
        );
    }

    #[test]
    fn rejects_bad_bundles() {
        let dir = tempfile::tempdir().unwrap();
        let app = dir.path().join("Payload/Demo.app");
        write_bundle(&app, &[("CFBundleName", "Demo".into())], false);
        let err = prepare(&app, "x.T", None, true).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Payload/Demo.app/Info.plist has no CFBundleIdentifier"
        );

        let app = sample_app(&dir.path().join("dup"));
        write_bundle(
            &app.join("PlugIns/Other.appex"),
            &[("CFBundleIdentifier", "net.other.widget".into())],
            false,
        );
        let err = prepare(&app, "com.example.demo.T", None, true).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Two bundles in the app would both be signed as com.example.demo.T.widget"
        );

        fs::write(app.join("Info.plist"), b"not a plist").unwrap();
        let err = prepare(&app, "x", None, true).unwrap_err();
        assert!(err.to_string().contains("is not a valid plist"), "{err}");
    }

    #[test]
    fn rejects_hostile_info_plists_before_parsing() {
        let dir = tempfile::tempdir().unwrap();
        let app = sample_app(dir.path());
        let deep = format!(
            "<plist><dict><key>CFBundleIdentifier</key><string>a.b</string><key>x</key>{}</dict></plist>",
            "<array>".repeat(100_000)
        );
        fs::write(app.join("Info.plist"), deep).unwrap();
        let err = prepare(&app, "x", None, true).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Payload/Demo.app/Info.plist is not a valid plist: nested more than 64 levels deep"
        );

        let mut big = plist_bytes(dict(&[("CFBundleIdentifier", "a.b".into())]), false);
        big.resize(MAX_PLIST_BYTES as usize + 1, b' ');
        fs::write(app.join("Info.plist"), big).unwrap();
        let err = prepare(&app, "x", None, true).unwrap_err();
        assert_eq!(
            err.to_string(),
            "Payload/Demo.app/Info.plist is larger than 4 MiB"
        );
    }
}
