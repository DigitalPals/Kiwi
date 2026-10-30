use std::{
    collections::{BTreeSet, HashMap},
    fmt, fs, io,
    path::{Path, PathBuf},
    process::Command,
    sync::atomic::{AtomicBool, Ordering},
    time::Duration,
};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

use crate::{
    Error, Identity, Profile, Result, archive,
    bundle::{self, CodeKind, remove_path, short},
    entitlements, macho, parse_profile, process, require_macos, select_profile_for,
};

/// Longest one `codesign` call may run. It blocks while macOS waits for an answer to a
/// keychain prompt, which would otherwise hold the signing slot forever.
const CODESIGN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Longest zipping the signed app may run (large apps on a busy disk).
const ZIP_TIMEOUT: Duration = Duration::from_secs(30 * 60);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SignRequest {
    pub input_ipa: PathBuf,
    /// Written atomically (via `<output>.partial`).
    pub output_ipa: PathBuf,
    /// Parent of a private temporary directory that is removed when signing ends.
    pub work_dir: PathBuf,
    pub identity: Identity,
    pub profiles: Vec<Profile>,
    /// New main bundle id; nested bundles are derived from it.
    pub bundle_id: String,
    pub url_scheme: Option<String>,
    pub strip_watch: bool,
    /// UDIDs the build should install on: profiles that include more of them are chosen
    /// first (see [`select_profile_for`]).
    #[serde(default)]
    pub devices: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SignStep {
    Extracting,
    Preparing,
    /// `done` of `total` items are signed; the next one is in progress.
    Signing {
        done: usize,
        total: usize,
    },
    Verifying,
    Packaging,
}

impl fmt::Display for SignStep {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Extracting => f.write_str("Extracting"),
            Self::Preparing => f.write_str("Preparing"),
            Self::Signing { done, total } => {
                write!(f, "Signing {} of {total}", (done + 1).min(*total))
            }
            Self::Verifying => f.write_str("Verifying"),
            Self::Packaging => f.write_str("Packaging"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SignReport {
    pub bundle_id: String,
    /// `CFBundleShortVersionString` (or `CFBundleVersion` when missing).
    pub version: String,
    pub build: Option<String>,
    /// `(original id, signed id)` of the main app, then nested apps and extensions.
    pub bundles: Vec<(String, String)>,
    pub profile_uuids: Vec<String>,
    /// Earliest expiry of the profiles used and the signing certificate.
    pub expires_at: DateTime<Utc>,
    /// Entitlement keys the original binaries requested (all bundles, sorted).
    pub original_entitlements: Vec<String>,
    /// Requested keys no profile could grant (all bundles, sorted).
    pub dropped_entitlements: Vec<String>,
    pub size: u64,
}

/// Re-signs `req.input_ipa` into `req.output_ipa`. Blocking; checks `cancel` between
/// steps and while external tools run, and reports steps through `progress`.
pub fn sign_ipa(
    req: &SignRequest,
    cancel: &AtomicBool,
    progress: &dyn Fn(SignStep),
) -> Result<SignReport> {
    require_macos()?;
    let tools = AppleTools {
        identity: &req.identity.sha1,
    };
    sign_with(&tools, req, cancel, progress, Utc::now())
}

/// The macOS tools, behind a trait so the rest of the flow is testable anywhere.
/// Paths are relative to `root` (the extracted IPA), which is the working directory.
trait Toolchain {
    fn sign(
        &self,
        root: &Path,
        path: &Path,
        entitlements: Option<&Path>,
        cancel: &AtomicBool,
    ) -> Result<()>;
    fn verify(&self, root: &Path, app: &Path, cancel: &AtomicBool) -> Result<()>;
    fn package(&self, root: &Path, output: &Path, cancel: &AtomicBool) -> Result<()>;
}

struct AppleTools<'a> {
    identity: &'a str,
}

impl Toolchain for AppleTools<'_> {
    fn sign(
        &self,
        root: &Path,
        path: &Path,
        entitlements: Option<&Path>,
        cancel: &AtomicBool,
    ) -> Result<()> {
        let mut cmd = Command::new("/usr/bin/codesign");
        cmd.args([
            "--force",
            "--timestamp=none",
            "--generate-entitlement-der",
            "-s",
            self.identity,
        ]);
        if let Some(entitlements) = entitlements {
            cmd.arg("--entitlements").arg(entitlements);
        }
        cmd.arg(path).current_dir(root);
        run(
            &mut cmd,
            &format!("Signing {}", path.display()),
            Some(path),
            CODESIGN_TIMEOUT,
            cancel,
        )
    }

    fn verify(&self, root: &Path, app: &Path, cancel: &AtomicBool) -> Result<()> {
        let mut cmd = Command::new("/usr/bin/codesign");
        cmd.args(["--verify", "--deep", "--strict"])
            .arg(app)
            .current_dir(root);
        run(
            &mut cmd,
            "Verifying the signature",
            Some(app),
            CODESIGN_TIMEOUT,
            cancel,
        )
    }

    fn package(&self, root: &Path, output: &Path, cancel: &AtomicBool) -> Result<()> {
        let mut cmd = Command::new("/usr/bin/zip");
        cmd.arg("-qry").arg(output).arg("Payload").current_dir(root);
        run(&mut cmd, "Packaging the IPA", None, ZIP_TIMEOUT, cancel)
    }
}

/// Runs a tool, killing it when `cancel` is set or after `timeout`; failures carry its
/// stderr, without the `<subject>: ` prefix codesign puts on its lines.
fn run(
    cmd: &mut Command,
    what: &str,
    subject: Option<&Path>,
    timeout: Duration,
    cancel: &AtomicBool,
) -> Result<()> {
    let out = process::output(cmd, what, timeout, cancel)?;
    if out.status.success() {
        return Ok(());
    }
    let prefix = subject.map(|s| format!("{}: ", s.display()));
    Err(Error::Tool(failure_message(
        what,
        &out.status.to_string(),
        &String::from_utf8_lossy(&out.stderr),
        prefix.as_deref(),
    )))
}

fn failure_message(what: &str, status: &str, stderr: &str, prefix: Option<&str>) -> String {
    let detail: Vec<&str> = stderr
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.ends_with(": replacing existing signature"))
        .map(|l| prefix.and_then(|p| l.strip_prefix(p)).unwrap_or(l))
        .collect();
    let mut message = if detail.is_empty() {
        format!("{what} failed ({status})")
    } else {
        let detail = detail.join("; ");
        let cut = detail
            .char_indices()
            .nth(1500)
            .map_or(detail.len(), |(i, _)| i);
        format!("{what} failed: {}", &detail[..cut])
    };
    if stderr.contains("errSecInternalComponent") {
        message.push_str(
            ". The signing key cannot be used from this session; Kiwi must run in the logged-in user's GUI session",
        );
    }
    message
}

struct Plan<'a> {
    profile: &'a Profile,
    entitlements: PathBuf,
}

fn sign_with(
    tools: &dyn Toolchain,
    req: &SignRequest,
    cancel: &AtomicBool,
    progress: &dyn Fn(SignStep),
    now: DateTime<Utc>,
) -> Result<SignReport> {
    validate_request(req)?;
    // Fail before extracting when the main app cannot be signed at all.
    select_profile_for(
        &req.profiles,
        &req.identity,
        &req.bundle_id,
        &req.devices,
        now,
    )?;
    let check = || {
        if cancel.load(Ordering::Relaxed) {
            Err(Error::Cancelled)
        } else {
            Ok(())
        }
    };
    check()?;
    fs::create_dir_all(&req.work_dir).map_err(Error::io_path("Could not create", &req.work_dir))?;
    let work = tempfile::Builder::new()
        .prefix("sign-")
        .tempdir_in(&req.work_dir)
        .map_err(Error::io_path(
            "Could not create a directory in",
            &req.work_dir,
        ))?;
    let root = work.path().join("root");

    progress(SignStep::Extracting);
    let app = archive::extract(&req.input_ipa, &root, cancel)?;
    check()?;

    progress(SignStep::Preparing);
    let prepared = bundle::prepare(
        &app,
        &req.bundle_id,
        req.url_scheme.as_deref(),
        req.strip_watch,
    )?;
    let mut plans = HashMap::new();
    let mut original_keys = BTreeSet::new();
    let mut dropped_keys = BTreeSet::new();
    let mut profiles: Vec<&Profile> = Vec::new();
    for (i, b) in prepared.bundles.iter().enumerate() {
        let profile =
            select_profile_for(&req.profiles, &req.identity, &b.new_id, &req.devices, now)?;
        let original = match &b.executable {
            Some(exe) => match macho::entitlements(exe) {
                Ok(ent) => ent.unwrap_or_default(),
                Err(e) if e.kind() == io::ErrorKind::NotFound => Default::default(),
                Err(e) => return Err(Error::io(format!("Could not read {}", short(exe)), e)),
            },
            None => Default::default(),
        };
        let (ent, dropped) = entitlements::build(profile, &b.new_id, &original);
        original_keys.extend(original.keys().cloned());
        dropped_keys.extend(dropped);
        let path = work.path().join(format!("entitlements-{i}.plist"));
        plist::to_file_xml(&path, &ent)
            .map_err(|e| Error::Tool(format!("Could not write entitlements: {e}")))?;
        if !profiles.iter().any(|p| p.uuid == profile.uuid) {
            profiles.push(profile);
        }
        plans.insert(
            b.path.clone(),
            Plan {
                profile,
                entitlements: path,
            },
        );
    }

    let rel = |path: &Path| {
        path.strip_prefix(&root)
            .map(Path::to_path_buf)
            .unwrap_or_else(|_| path.to_path_buf())
    };
    let items: Vec<(&Path, CodeKind)> = prepared
        .code
        .iter()
        .map(|c| (c.path.as_path(), c.kind))
        .chain([(app.as_path(), CodeKind::Bundle)])
        .collect();
    let total = items.len();
    for (done, (path, kind)) in items.into_iter().enumerate() {
        check()?;
        progress(SignStep::Signing { done, total });
        let entitlements = if kind == CodeKind::Bundle {
            let plan = plans.get(path).ok_or_else(|| {
                Error::InvalidIpa(format!("{} was not prepared for signing", short(path)))
            })?;
            embed_profile(plan.profile, path)?;
            Some(plan.entitlements.as_path())
        } else {
            None
        };
        tools.sign(&root, &rel(path), entitlements, cancel)?;
    }

    check()?;
    progress(SignStep::Verifying);
    bundle::normalize_permissions(&root.join("Payload"))?;
    tools.verify(&root, &rel(&app), cancel)?;

    check()?;
    progress(SignStep::Packaging);
    let size = package(tools, &root, &req.output_ipa, cancel)?;

    let expires_at = profiles
        .iter()
        .map(|p| p.expires_at)
        .chain(req.identity.expires_at)
        .min()
        .unwrap_or(now);
    Ok(SignReport {
        bundle_id: req.bundle_id.clone(),
        version: prepared.version,
        build: prepared.build,
        bundles: prepared
            .bundles
            .into_iter()
            .map(|b| (b.old_id, b.new_id))
            .collect(),
        profile_uuids: profiles.iter().map(|p| p.uuid.clone()).collect(),
        expires_at,
        original_entitlements: original_keys.into_iter().collect(),
        dropped_entitlements: dropped_keys.into_iter().collect(),
        size,
    })
}

fn validate_request(req: &SignRequest) -> Result<()> {
    let id = &req.bundle_id;
    let valid_id = !id.is_empty()
        && id.len() <= 255
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'.' || b == b'-')
        && !id.starts_with('.')
        && !id.ends_with('.')
        && !id.contains("..");
    if !valid_id {
        return Err(Error::InvalidRequest(format!("Invalid bundle id: {id:?}")));
    }
    if let Some(scheme) = &req.url_scheme {
        let mut bytes = scheme.bytes();
        let valid = bytes.next().is_some_and(|b| b.is_ascii_alphabetic())
            && bytes.all(|b| b.is_ascii_alphanumeric() || b"+-.".contains(&b));
        if !valid {
            return Err(Error::InvalidRequest(format!(
                "Invalid URL scheme: {scheme:?}"
            )));
        }
    }
    let sha1 = &req.identity.sha1;
    if sha1.len() != 40 || !sha1.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(Error::InvalidRequest(format!(
            "Invalid signing identity: {sha1:?}"
        )));
    }
    Ok(())
}

/// Copies the profile into the bundle, checking it is still the profile that was chosen.
fn embed_profile(profile: &Profile, bundle: &Path) -> Result<()> {
    let bytes = fs::read(&profile.path).map_err(Error::io_path("Could not read", &profile.path))?;
    if parse_profile(&bytes)?.uuid != profile.uuid {
        return Err(Error::InvalidProfile(format!(
            "{} changed since it was loaded; rescan the profiles",
            profile.path.display()
        )));
    }
    let dest = bundle.join("embedded.mobileprovision");
    remove_path(&dest)?;
    fs::write(&dest, bytes).map_err(|e| Error::io(format!("Could not write {}", short(&dest)), e))
}

/// Zips `root/Payload` to `<output>.partial`, flushes it to disk, then renames it to
/// `output`.
fn package(tools: &dyn Toolchain, root: &Path, output: &Path, cancel: &AtomicBool) -> Result<u64> {
    let output =
        std::path::absolute(output).map_err(Error::io_path("Invalid output path", output))?;
    let (Some(parent), Some(name)) = (output.parent(), output.file_name()) else {
        return Err(Error::InvalidRequest(format!(
            "Invalid output path: {}",
            output.display()
        )));
    };
    fs::create_dir_all(parent).map_err(Error::io_path("Could not create", parent))?;
    let partial = parent.join(format!("{}.partial", name.to_string_lossy()));
    remove_path(&partial)?;
    let result = tools
        .package(root, &partial, cancel)
        .and_then(|()| {
            fs::File::open(&partial)
                .and_then(|f| f.sync_all())
                .map_err(Error::io_path("Could not write", &partial))
        })
        .and_then(|()| {
            fs::rename(&partial, &output).map_err(Error::io_path("Could not write", &output))
        });
    if result.is_err() {
        let _ = fs::remove_file(&partial);
    }
    result?;
    fs::metadata(&output)
        .map(|m| m.len())
        .map_err(Error::io_path("Could not read", &output))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{
        ProfileSpec, TEAM, Zip, entitlements_xml, identity, macho, plist_bytes, profile_bytes,
    };
    use chrono::TimeZone;
    use plist::{Dictionary, Value};
    use std::{cell::RefCell, os::unix::fs::PermissionsExt, sync::Mutex};

    const ARM64: u32 = 0x0100_000c;

    #[derive(Default)]
    struct Fake {
        calls: RefCell<Vec<String>>,
        fail_verify: bool,
    }

    impl Toolchain for Fake {
        fn sign(
            &self,
            root: &Path,
            path: &Path,
            entitlements: Option<&Path>,
            _: &AtomicBool,
        ) -> Result<()> {
            assert!(path.is_relative());
            let mut call = format!("sign {}", path.display());
            if let Some(ent) = entitlements {
                assert!(root.join(path).join("embedded.mobileprovision").is_file());
                let ent: Dictionary = plist::from_file(ent).unwrap();
                let app_id = ent
                    .get("application-identifier")
                    .and_then(Value::as_string)
                    .unwrap();
                call.push_str(&format!(" as {app_id}"));
            }
            self.calls.borrow_mut().push(call);
            Ok(())
        }

        fn verify(&self, _: &Path, app: &Path, _: &AtomicBool) -> Result<()> {
            self.calls
                .borrow_mut()
                .push(format!("verify {}", app.display()));
            if self.fail_verify {
                return Err(Error::Tool("Verifying the signature failed: nope".into()));
            }
            Ok(())
        }

        fn package(&self, root: &Path, output: &Path, _: &AtomicBool) -> Result<()> {
            assert!(output.is_absolute() && output.to_string_lossy().ends_with(".ipa.partial"));
            let mode = |p: &str| fs::metadata(root.join(p)).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode("Payload/Demo.app/Info.plist"), 0o644);
            assert_eq!(mode("Payload/Demo.app/Demo"), 0o755);
            assert_eq!(mode("Payload/Demo.app/PlugIns"), 0o755);
            fs::write(output, b"signed ipa").unwrap();
            self.calls.borrow_mut().push("package".into());
            Ok(())
        }
    }

    fn info(entries: &[(&str, Value)]) -> Vec<u8> {
        plist_bytes(
            entries
                .iter()
                .map(|(k, v)| ((*k).to_owned(), v.clone()))
                .collect(),
            true,
        )
    }

    fn strings(items: &[&str]) -> Value {
        Value::Array(items.iter().map(|s| (*s).into()).collect())
    }

    struct Setup {
        dir: tempfile::TempDir,
        req: SignRequest,
    }

    fn setup() -> Setup {
        let dir = tempfile::tempdir().unwrap();
        let main_ent = entitlements_xml(&[
            ("application-identifier", "OLDTEAM.com.example.demo".into()),
            (
                "com.apple.security.application-groups",
                strings(&["group.com.example.demo"]),
            ),
            ("aps-environment", "production".into()),
            ("com.apple.private.foo", true.into()),
        ]);
        let share_ent = entitlements_xml(&[(
            "com.apple.security.application-groups",
            strings(&["group.com.example.demo"]),
        )]);
        let ipa = Zip::new()
            .file(
                "Payload/Demo.app/Info.plist",
                &info(&[
                    ("CFBundleIdentifier", "com.example.demo".into()),
                    ("CFBundleExecutable", "Demo".into()),
                    ("CFBundleShortVersionString", "1.2".into()),
                    ("CFBundleVersion", "42".into()),
                ]),
                0o644,
            )
            .file(
                "Payload/Demo.app/Demo",
                &macho(ARM64, Some(&main_ent)),
                0o755,
            )
            .file("Payload/Demo.app/SC_Info/Demo.sinf", b"drm", 0o644)
            .file(
                "Payload/Demo.app/Frameworks/Lib.dylib",
                &macho(ARM64, None),
                0o755,
            )
            .file(
                "Payload/Demo.app/Frameworks/Kit.framework/Kit",
                &macho(ARM64, None),
                0o755,
            )
            .file(
                "Payload/Demo.app/PlugIns/Share.appex/Info.plist",
                &info(&[
                    ("CFBundleIdentifier", "com.example.demo.Share".into()),
                    ("CFBundleExecutable", "Share".into()),
                ]),
                0o644,
            )
            .file(
                "Payload/Demo.app/PlugIns/Share.appex/Share",
                &macho(ARM64, Some(&share_ent)),
                0o755,
            )
            .file(
                "Payload/Demo.app/Watch/W.app/Info.plist",
                &info(&[("CFBundleIdentifier", "com.example.demo.watchkitapp".into())]),
                0o644,
            )
            .file("iTunesMetadata.plist", b"meta", 0o644)
            .finish();
        let input = dir.path().join("in.ipa");
        fs::write(&input, ipa).unwrap();

        let bundle_id = format!("com.example.demo.{TEAM}");
        let mut explicit = ProfileSpec::explicit(&bundle_id);
        explicit.entitlements = vec![
            (
                "com.apple.security.application-groups",
                strings(&["group.com.example.kiwi"]),
            ),
            ("aps-environment", "development".into()),
        ];
        let mut profiles = Vec::new();
        for spec in [ProfileSpec::wildcard(), explicit] {
            let path = dir.path().join(format!("{}.mobileprovision", spec.uuid));
            fs::write(&path, profile_bytes(&spec)).unwrap();
            profiles.push(Profile {
                path,
                ..crate::testutil::profile(&spec)
            });
        }
        let req = SignRequest {
            input_ipa: input,
            output_ipa: dir.path().join("out/Demo.ipa"),
            work_dir: dir.path().join("work"),
            identity: identity(),
            profiles,
            url_scheme: Some(crate::url_scheme_for(&bundle_id)),
            bundle_id,
            strip_watch: true,
            devices: Vec::new(),
        };
        Setup { dir, req }
    }

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 10, 29, 12, 0, 0).unwrap()
    }

    #[test]
    fn signs_inside_out_with_profiles_and_entitlements() {
        let Setup { dir, req } = setup();
        let fake = Fake::default();
        let steps = Mutex::new(Vec::new());
        let report = sign_with(
            &fake,
            &req,
            &AtomicBool::new(false),
            &|s| steps.lock().unwrap().push(s),
            now(),
        )
        .unwrap();

        let id = format!("com.example.demo.{TEAM}");
        assert_eq!(
            *fake.calls.borrow(),
            vec![
                "sign Payload/Demo.app/Frameworks/Kit.framework".to_owned(),
                "sign Payload/Demo.app/Frameworks/Lib.dylib".to_owned(),
                format!("sign Payload/Demo.app/PlugIns/Share.appex as {TEAM}.{id}.Share"),
                format!("sign Payload/Demo.app as {TEAM}.{id}"),
                "verify Payload/Demo.app".to_owned(),
                "package".to_owned(),
            ]
        );
        assert_eq!(
            *steps.lock().unwrap(),
            vec![
                SignStep::Extracting,
                SignStep::Preparing,
                SignStep::Signing { done: 0, total: 4 },
                SignStep::Signing { done: 1, total: 4 },
                SignStep::Signing { done: 2, total: 4 },
                SignStep::Signing { done: 3, total: 4 },
                SignStep::Verifying,
                SignStep::Packaging,
            ]
        );
        assert_eq!(
            report,
            SignReport {
                bundle_id: id.clone(),
                version: "1.2".into(),
                build: Some("42".into()),
                bundles: vec![
                    ("com.example.demo".into(), id.clone()),
                    ("com.example.demo.Share".into(), format!("{id}.Share")),
                ],
                profile_uuids: vec![format!("explicit-{id}"), ProfileSpec::wildcard().uuid],
                // The certificate expires before the profiles.
                expires_at: Utc.with_ymd_and_hms(2027, 10, 25, 12, 0, 0).unwrap(),
                original_entitlements: vec![
                    "application-identifier".into(),
                    "aps-environment".into(),
                    "com.apple.private.foo".into(),
                    "com.apple.security.application-groups".into(),
                ],
                dropped_entitlements: vec![
                    "com.apple.private.foo".into(),
                    "com.apple.security.application-groups".into(),
                ],
                size: 10,
            }
        );
        assert_eq!(fs::read(&req.output_ipa).unwrap(), b"signed ipa");
        assert!(!dir.path().join("out/Demo.ipa.partial").exists());
        assert_eq!(
            fs::read_dir(&req.work_dir).unwrap().count(),
            0,
            "work directory is cleaned up"
        );
    }

    #[test]
    fn writes_entitlements_and_profiles_into_the_bundle() {
        struct Inspect(RefCell<Vec<(PathBuf, Dictionary, Vec<u8>)>>);
        impl Toolchain for Inspect {
            fn sign(
                &self,
                root: &Path,
                path: &Path,
                ent: Option<&Path>,
                _: &AtomicBool,
            ) -> Result<()> {
                if let Some(ent) = ent {
                    let embedded =
                        fs::read(root.join(path).join("embedded.mobileprovision")).unwrap();
                    self.0.borrow_mut().push((
                        path.to_owned(),
                        plist::from_file(ent).unwrap(),
                        embedded,
                    ));
                }
                if path.ends_with("Demo.app") {
                    let info: Dictionary =
                        plist::from_file(root.join(path).join("Info.plist")).unwrap();
                    assert!(info.contains_key("CFBundleURLTypes"));
                    assert!(!root.join(path).join("SC_Info").exists());
                    assert!(!root.join(path).join("Watch").exists());
                }
                Ok(())
            }
            fn verify(&self, _: &Path, _: &Path, _: &AtomicBool) -> Result<()> {
                Ok(())
            }
            fn package(&self, _: &Path, output: &Path, _: &AtomicBool) -> Result<()> {
                fs::write(output, b"").map_err(|e| Error::io("x", e))
            }
        }
        let Setup { req, dir: _dir } = setup();
        let inspect = Inspect(RefCell::default());
        sign_with(&inspect, &req, &AtomicBool::new(false), &|_| {}, now()).unwrap();
        let signed = inspect.0.borrow();
        let (_, share, share_profile) = &signed[0];
        let (_, main, main_profile) = &signed[1];
        assert_eq!(
            parse_profile(share_profile).unwrap().uuid,
            ProfileSpec::wildcard().uuid
        );
        assert_eq!(*main_profile, fs::read(&req.profiles[1].path).unwrap());
        assert!(!share.contains_key("com.apple.security.application-groups"));
        assert_eq!(main.get("aps-environment"), Some(&"development".into()));
        assert_eq!(
            main.get("com.apple.security.application-groups"),
            Some(&strings(&["group.com.example.kiwi"]))
        );
        assert_eq!(main.get("get-task-allow"), Some(&true.into()));
        assert!(!main.contains_key("com.apple.private.foo"));
    }

    #[test]
    fn cancels_between_steps() {
        let Setup { req, dir: _dir } = setup();
        let fake = Fake::default();
        let cancel = AtomicBool::new(false);
        let progress = |step| {
            if step == (SignStep::Signing { done: 1, total: 4 }) {
                cancel.store(true, Ordering::Relaxed);
            }
        };
        let err = sign_with(&fake, &req, &cancel, &progress, now()).unwrap_err();
        assert!(matches!(err, Error::Cancelled));
        assert_eq!(fake.calls.borrow().len(), 2);
        assert!(!req.output_ipa.exists());
        assert_eq!(fs::read_dir(&req.work_dir).unwrap().count(), 0);
    }

    #[test]
    fn fails_early_without_a_profile() {
        let Setup { mut req, dir: _dir } = setup();
        req.profiles.clear();
        let steps = RefCell::new(Vec::new());
        let err = sign_with(
            &Fake::default(),
            &req,
            &AtomicBool::new(false),
            &|s| steps.borrow_mut().push(s),
            now(),
        )
        .unwrap_err();
        assert!(matches!(err, Error::NoProfile(_)));
        assert!(steps.borrow().is_empty());

        let Setup { mut req, dir: _dir } = setup();
        req.identity.sha1 = "0".repeat(40);
        let err = sign_with(
            &Fake::default(),
            &req,
            &AtomicBool::new(false),
            &|_| {},
            now(),
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("does not include the certificate"),
            "{err}"
        );
    }

    #[test]
    fn tool_failure_leaves_no_output() {
        let Setup { req, dir: _dir } = setup();
        let fake = Fake {
            fail_verify: true,
            ..Fake::default()
        };
        let err = sign_with(&fake, &req, &AtomicBool::new(false), &|_| {}, now()).unwrap_err();
        assert_eq!(err.to_string(), "Verifying the signature failed: nope");
        assert!(!req.output_ipa.exists());
    }

    #[test]
    fn detects_changed_profile_files() {
        let Setup { req, dir: _dir } = setup();
        fs::write(
            &req.profiles[1].path,
            profile_bytes(&ProfileSpec::wildcard()),
        )
        .unwrap();
        let err = sign_with(
            &Fake::default(),
            &req,
            &AtomicBool::new(false),
            &|_| {},
            now(),
        )
        .unwrap_err();
        assert!(
            err.to_string().contains("changed since it was loaded"),
            "{err}"
        );
    }

    #[test]
    fn validates_requests() {
        let Setup { req, dir: _dir } = setup();
        let bad = |f: &dyn Fn(&mut SignRequest)| {
            let mut r = req.clone();
            f(&mut r);
            sign_with(
                &Fake::default(),
                &r,
                &AtomicBool::new(false),
                &|_| {},
                now(),
            )
            .unwrap_err()
            .to_string()
        };
        assert_eq!(
            bad(&|r| r.bundle_id = "com.example/../x".into()),
            "Invalid bundle id: \"com.example/../x\""
        );
        assert!(bad(&|r| r.bundle_id = ".x".into()).starts_with("Invalid bundle id"));
        assert!(bad(&|r| r.url_scheme = Some("1abc".into())).starts_with("Invalid URL scheme"));
        assert!(bad(&|r| r.url_scheme = Some("a<b>".into())).starts_with("Invalid URL scheme"));
        assert!(bad(&|r| r.identity.sha1 = "abc".into()).starts_with("Invalid signing identity"));
    }

    #[test]
    fn step_display() {
        assert_eq!(
            SignStep::Signing {
                done: 11,
                total: 23
            }
            .to_string(),
            "Signing 12 of 23"
        );
        assert_eq!(
            SignStep::Signing {
                done: 23,
                total: 23
            }
            .to_string(),
            "Signing 23 of 23"
        );
        assert_eq!(SignStep::Packaging.to_string(), "Packaging");
    }

    #[test]
    fn tool_failure_messages() {
        let msg = failure_message(
            "Signing Payload/A.app",
            "exit status: 1",
            "Payload/A.app: replacing existing signature\nPayload/A.app: errSecInternalComponent\n",
            Some("Payload/A.app: "),
        );
        assert!(
            msg.starts_with(
                "Signing Payload/A.app failed: errSecInternalComponent. The signing key"
            ),
            "{msg}"
        );
        assert_eq!(
            failure_message("Packaging the IPA", "exit status: 15", "", None),
            "Packaging the IPA failed (exit status: 15)"
        );
    }

    #[test]
    fn runs_cancels_and_times_out_tools() {
        let minute = Duration::from_secs(60);
        assert!(
            run(
                Command::new("/bin/sh").args(["-c", "exit 0"]),
                "ok",
                None,
                minute,
                &AtomicBool::new(false)
            )
            .is_ok()
        );
        let err = run(
            Command::new("/bin/sh").args(["-c", "echo boom >&2; exit 3"]),
            "Tool",
            None,
            minute,
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert_eq!(err.to_string(), "Tool failed: boom");
        let started = std::time::Instant::now();
        let err = run(
            Command::new("/bin/sleep").arg("10"),
            "sleep",
            None,
            minute,
            &AtomicBool::new(true),
        )
        .unwrap_err();
        assert!(matches!(err, Error::Cancelled));
        let err = run(
            Command::new("/bin/sleep").arg("10"),
            "Sleeping",
            None,
            Duration::from_millis(100),
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert!(
            err.to_string()
                .starts_with("Sleeping did not finish within 100 ms"),
            "{err}"
        );
        assert!(started.elapsed() < Duration::from_secs(5));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn unsupported_off_macos() {
        let Setup { req, dir: _dir } = setup();
        let err = sign_ipa(&req, &AtomicBool::new(false), &|_| {}).unwrap_err();
        assert!(matches!(err, Error::Unsupported));
    }
}
