//! Validates an IPA and extracts its `Payload/*.app`.
//!
//! Every entry is checked before anything is written: no absolute paths, no `..`, no
//! symlink targets leaving the extraction root, no entries below a symlink, exactly one
//! `Payload/*.app`. Symlinks are created after all files, so nothing is written through
//! one, and each is resolved afterwards to catch chains that escape.

use std::{
    collections::{BTreeSet, HashSet},
    fs::{self, File, OpenOptions},
    io::{self, BufReader, Read},
    os::unix::fs::{OpenOptionsExt, symlink},
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

use zip::ZipArchive;

use crate::{Error, Result};

const MAX_ENTRIES: usize = 500_000;
const MAX_TOTAL_SIZE: u64 = 16 << 30;
const MAX_LINK_TARGET: u64 = 4096;

enum Kind {
    Dir,
    File { executable: bool },
    Symlink(String),
}

struct Entry {
    index: usize,
    parts: Vec<String>,
    kind: Kind,
}

fn invalid(message: impl Into<String>) -> Error {
    Error::InvalidIpa(message.into())
}

fn zip_error(e: zip::result::ZipError) -> Error {
    invalid(format!("The IPA could not be read: {e}"))
}

/// Extracts the app of `ipa` into `dest` (which gets a `Payload` directory) and
/// returns the path of the `.app`. Entries outside `Payload/` are not extracted.
pub(crate) fn extract(ipa: &Path, dest: &Path, cancel: &AtomicBool) -> Result<PathBuf> {
    let file = File::open(ipa).map_err(Error::io_path("Could not open", ipa))?;
    let mut zip = ZipArchive::new(BufReader::with_capacity(1 << 18, file)).map_err(zip_error)?;
    let (app, entries) = validate(&mut zip)?;
    let write_error = |path: &Path| Error::io_path("Could not extract", path);
    fs::create_dir_all(dest).map_err(write_error(dest))?;
    let mut written = 0u64;
    let mut links = Vec::new();
    for entry in &entries {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let path = entry
            .parts
            .iter()
            .fold(dest.to_path_buf(), |p, c| p.join(c));
        let parent = path.parent().unwrap_or(dest);
        match &entry.kind {
            Kind::Dir => fs::create_dir_all(&path).map_err(write_error(&path))?,
            Kind::File { executable } => {
                fs::create_dir_all(parent).map_err(write_error(parent))?;
                let mut out = OpenOptions::new()
                    .write(true)
                    .create_new(true)
                    .mode(if *executable { 0o755 } else { 0o644 })
                    .open(&path)
                    .map_err(write_error(&path))?;
                let mut data = zip.by_index(entry.index).map_err(zip_error)?;
                let limit = MAX_TOTAL_SIZE - written;
                written += io::copy(&mut (&mut data).take(limit + 1), &mut out)
                    .map_err(write_error(&path))?;
                if written > MAX_TOTAL_SIZE {
                    return Err(too_large());
                }
            }
            Kind::Symlink(target) => links.push((entry, target)),
        }
    }
    let root = fs::canonicalize(dest).map_err(Error::io_path("Could not resolve", dest))?;
    let mut created = Vec::new();
    for (entry, target) in links {
        let Some((name, parents)) = entry.parts.split_last() else {
            continue;
        };
        let path = real_dir(dest, parents)?.join(name);
        symlink(target, &path).map_err(write_error(&path))?;
        created.push((path, entry));
    }
    for (path, entry) in created {
        if !fs::canonicalize(path).is_ok_and(|resolved| resolved.starts_with(&root)) {
            return Err(invalid(format!(
                "The IPA contains a symbolic link that points outside the app or to a missing file: {}",
                entry.parts.join("/")
            )));
        }
    }
    Ok(dest.join("Payload").join(app))
}

/// Creates `root/parts…` one component at a time, requiring every existing component
/// to be a real directory. lstat resolves names like the filesystem does, so a symlink
/// that differs only in case or Unicode normalization is caught too.
fn real_dir(root: &Path, parts: &[String]) -> Result<PathBuf> {
    let mut path = root.to_path_buf();
    for part in parts {
        path.push(part);
        match fs::symlink_metadata(&path) {
            Ok(meta) if meta.is_dir() => {}
            Ok(_) => {
                return Err(invalid(format!(
                    "The IPA contains a file inside a symbolic link: {}",
                    parts.join("/")
                )));
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                fs::create_dir(&path).map_err(Error::io_path("Could not extract", &path))?;
            }
            Err(e) => return Err(Error::io_path("Could not extract", &path)(e)),
        }
    }
    Ok(path)
}

fn too_large() -> Error {
    invalid(format!(
        "The IPA is too large to extract (over {} GB)",
        MAX_TOTAL_SIZE >> 30
    ))
}

fn validate<R: Read + io::Seek>(zip: &mut ZipArchive<R>) -> Result<(String, Vec<Entry>)> {
    if zip.len() > MAX_ENTRIES {
        return Err(invalid("The IPA contains too many files"));
    }
    let mut entries = Vec::new();
    let mut apps = BTreeSet::new();
    let mut total = 0u64;
    for index in 0..zip.len() {
        let mut file = zip.by_index(index).map_err(zip_error)?;
        let name = file.name().to_owned();
        let parts = components(&name)
            .ok_or_else(|| invalid(format!("The IPA contains an unsafe path: {name}")))?;
        total = total.saturating_add(file.size());
        let kind = if file.is_symlink() {
            let mut target = String::new();
            (&mut file)
                .take(MAX_LINK_TARGET)
                .read_to_string(&mut target)
                .map_err(|e| invalid(format!("The IPA could not be read: {e}")))?;
            if escapes(&parts, &target) {
                return Err(invalid(format!(
                    "The IPA contains a symbolic link that points outside the app: {name} -> {target}"
                )));
            }
            Kind::Symlink(target)
        } else if file.is_dir() {
            Kind::Dir
        } else {
            Kind::File {
                executable: file.unix_mode().is_none_or(|mode| mode & 0o111 != 0),
            }
        };
        let [first, top, rest @ ..] = parts.as_slice() else {
            continue;
        };
        if first != "Payload" || !top.ends_with(".app") {
            continue; // iTunesMetadata.plist, META-INF, Payload/.DS_Store, …
        }
        if rest.is_empty() && !matches!(kind, Kind::Dir) {
            return Err(invalid(format!(
                "Payload/{top} in the IPA is not a directory"
            )));
        }
        apps.insert(top.clone());
        entries.push(Entry { index, parts, kind });
    }
    if total > MAX_TOTAL_SIZE {
        return Err(too_large());
    }
    let app = match apps.len() {
        0 => return Err(invalid("The IPA contains no app (Payload/*.app)")),
        1 => apps.pop_first().unwrap_or_default(),
        _ => {
            let names: Vec<_> = apps.into_iter().collect();
            return Err(invalid(format!(
                "The IPA contains more than one app: {}",
                names.join(", ")
            )));
        }
    };
    let links: HashSet<&[String]> = entries
        .iter()
        .filter(|e| matches!(e.kind, Kind::Symlink(_)))
        .map(|e| e.parts.as_slice())
        .collect();
    if let Some(entry) = entries
        .iter()
        .find(|e| (1..e.parts.len()).any(|n| links.contains(&e.parts[..n])))
    {
        return Err(invalid(format!(
            "The IPA contains a file inside a symbolic link: {}",
            entry.parts.join("/")
        )));
    }
    Ok((app, entries))
}

/// Path components of a zip entry name; `None` for absolute paths, `..` or NUL.
fn components(name: &str) -> Option<Vec<String>> {
    if name.starts_with('/') || name.contains('\0') {
        return None;
    }
    let mut parts = Vec::new();
    for part in name.split('/') {
        match part {
            "" | "." => {}
            ".." => return None,
            part => parts.push(part.to_owned()),
        }
    }
    Some(parts)
}

/// Whether a symlink at `link` pointing to `target` leaves the root, lexically.
fn escapes(link: &[String], target: &str) -> bool {
    if target.is_empty() || target.starts_with('/') || target.contains('\0') {
        return true;
    }
    let mut stack: Vec<&str> = link[..link.len().saturating_sub(1)]
        .iter()
        .map(String::as_str)
        .collect();
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::Zip;
    use std::os::unix::fs::PermissionsExt;

    fn run(zip: Zip) -> (tempfile::TempDir, Result<PathBuf>) {
        let dir = tempfile::tempdir().unwrap();
        let ipa = dir.path().join("app.ipa");
        fs::write(&ipa, zip.finish()).unwrap();
        let result = extract(&ipa, &dir.path().join("out"), &AtomicBool::new(false));
        (dir, result)
    }

    fn error(zip: Zip) -> String {
        run(zip).1.unwrap_err().to_string()
    }

    fn app() -> Zip {
        Zip::new().dir("Payload/").dir("Payload/Demo.app/").file(
            "Payload/Demo.app/Info.plist",
            b"plist",
            0o644,
        )
    }

    #[test]
    fn extracts_payload_with_modes_and_links() {
        let zip = app()
            .file("Payload/Demo.app/Demo", b"binary", 0o755)
            .file(
                "Payload/Demo.app/Frameworks/Kit.framework/Kit",
                b"kit",
                0o755,
            )
            .symlink("Payload/Demo.app/Current", "Frameworks/Kit.framework")
            .symlink(
                "Payload/Demo.app/Frameworks/Kit.framework/Self",
                "../../../Demo.app/Demo",
            )
            .file("Payload/.DS_Store", b"", 0o644)
            .file("iTunesMetadata.plist", b"meta", 0o644)
            .file("META-INF/com.apple.ZipMetadata.plist", b"meta", 0o644);
        let (dir, result) = run(zip);
        let app = result.unwrap();
        let out = dir.path().join("out");
        assert_eq!(app, out.join("Payload/Demo.app"));
        let mode = |p: &str| fs::metadata(app.join(p)).unwrap().permissions().mode() & 0o777;
        assert_ne!(mode("Demo") & 0o100, 0);
        assert_eq!(mode("Info.plist") & 0o111, 0);
        assert_eq!(
            fs::read_link(app.join("Current")).unwrap(),
            Path::new("Frameworks/Kit.framework")
        );
        assert_eq!(fs::read(app.join("Current/Kit")).unwrap(), b"kit");
        assert!(!out.join("iTunesMetadata.plist").exists());
        assert!(!out.join("META-INF").exists());
        assert!(!out.join("Payload/.DS_Store").exists());
    }

    #[test]
    fn rejects_unsafe_paths() {
        let err = error(app().file("/tmp/evil", b"x", 0o644));
        assert_eq!(err, "The IPA contains an unsafe path: /tmp/evil");
        let err = error(app().file("Payload/Demo.app/../../evil", b"x", 0o644));
        assert!(err.contains("unsafe path"), "{err}");
        let err = error(app().file("META-INF/../../evil", b"x", 0o644));
        assert!(err.contains("unsafe path"), "{err}");
    }

    #[test]
    fn rejects_escaping_symlinks() {
        let err = error(app().symlink("Payload/Demo.app/etc", "/etc"));
        assert!(
            err.contains("points outside the app: Payload/Demo.app/etc -> /etc"),
            "{err}"
        );
        let err = error(app().symlink("Payload/Demo.app/up", "../../../outside"));
        assert!(err.contains("points outside the app"), "{err}");
        let err = error(app().symlink("Payload/Demo.app/self", ""));
        assert!(err.contains("points outside the app"), "{err}");
    }

    #[test]
    fn rejects_entries_below_symlinks() {
        let err = error(app().symlink("Payload/Demo.app/link", "Frameworks").file(
            "Payload/Demo.app/link/evil",
            b"x",
            0o644,
        ));
        assert_eq!(
            err,
            "The IPA contains a file inside a symbolic link: Payload/Demo.app/link/evil"
        );
    }

    #[test]
    fn symlinks_are_never_created_through_symlinks() {
        // Simulates a case-insensitive name clash: "l" already exists as a symlink when
        // the entry below it is created, which the exact-name check cannot see.
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir_all(dir.path().join("Payload/Demo.app")).unwrap();
        symlink("..", dir.path().join("Payload/Demo.app/l")).unwrap();
        let parts = ["Payload", "Demo.app", "l"].map(String::from);
        let err = real_dir(dir.path(), &parts).unwrap_err();
        assert_eq!(
            err.to_string(),
            "The IPA contains a file inside a symbolic link: Payload/Demo.app/l"
        );
        let created = real_dir(
            dir.path(),
            &["Payload", "Demo.app", "new", "dir"].map(String::from),
        )
        .unwrap();
        assert!(created.is_dir() && created.ends_with("Payload/Demo.app/new/dir"));
    }

    #[test]
    fn case_clashing_symlinks_stay_inside() {
        // On case-insensitive APFS "l" is "L"; on Linux they differ. Either way extraction
        // fails and nothing is created outside the destination.
        let zip = app()
            .symlink("Payload/Demo.app/L", "..")
            .symlink("Payload/Demo.app/l/M", "../../..")
            .symlink("Payload/Demo.app/l/m/evil", "x");
        let (dir, result) = run(zip);
        assert!(result.is_err());
        let mut names: Vec<_> = fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name())
            .collect();
        names.sort();
        assert_eq!(names, ["app.ipa", "out"]);
        assert!(
            !dir.path().join("out/evil").exists() && !dir.path().join("out/Payload/evil").exists()
        );
    }

    #[test]
    fn rejects_symlink_chains_that_escape() {
        // Each target stays inside lexically, but s2 resolves through s1 to the root's parent.
        let zip = app()
            .file("Payload/Demo.app/d1/d2/file", b"x", 0o644)
            .symlink("Payload/Demo.app/d1/d2/s1", "../../..")
            .symlink("Payload/Demo.app/s2", "d1/d2/s1/../..");
        let err = error(zip);
        assert!(
            err.contains("points outside the app or to a missing file: Payload/Demo.app/s2"),
            "{err}"
        );
        let err = error(app().symlink("Payload/Demo.app/missing", "nothing"));
        assert!(err.contains("missing file"), "{err}");
    }

    #[test]
    fn requires_exactly_one_app() {
        let err = error(Zip::new().file("Payload/readme.txt", b"x", 0o644));
        assert_eq!(err, "The IPA contains no app (Payload/*.app)");
        let err = error(app().file("Payload/Other.app/Info.plist", b"x", 0o644));
        assert_eq!(
            err,
            "The IPA contains more than one app: Demo.app, Other.app"
        );
        let err = error(Zip::new().file("Payload/Demo.app", b"x", 0o644));
        assert_eq!(err, "Payload/Demo.app in the IPA is not a directory");
        let err = error(Zip::new().symlink("Payload/Demo.app", "../x"));
        assert!(err.contains("not a directory"), "{err}");
    }

    #[test]
    fn rejects_non_zip_and_honours_cancel() {
        let dir = tempfile::tempdir().unwrap();
        let ipa = dir.path().join("app.ipa");
        fs::write(&ipa, b"not a zip").unwrap();
        let err = extract(&ipa, &dir.path().join("out"), &AtomicBool::new(false)).unwrap_err();
        assert!(
            err.to_string().starts_with("The IPA could not be read"),
            "{err}"
        );
        fs::write(&ipa, app().finish()).unwrap();
        let err = extract(&ipa, &dir.path().join("out"), &AtomicBool::new(true)).unwrap_err();
        assert!(matches!(err, Error::Cancelled));
    }

    #[test]
    fn lexical_escape_rules() {
        let link = |s: &str| s.split('/').map(str::to_owned).collect::<Vec<_>>();
        assert!(!escapes(&link("Payload/A.app/l"), "../../x"));
        assert!(escapes(&link("Payload/A.app/l"), "../../../x"));
        assert!(!escapes(&link("Payload/A.app/l"), "./a/../b"));
        assert!(escapes(&link("l"), ".."));
        assert_eq!(components("a//./b/"), Some(vec!["a".into(), "b".into()]));
        assert_eq!(components("a/../b"), None);
    }
}
