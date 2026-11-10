//! The files backend: signs with rcodesign's library (`apple-codesign`) and an uploaded
//! `.p12` identity, checks the result in Rust and zips it with [`package`].
//!
//! Bundles are sealed the way `codesign` seals iOS bundles: every file except the main
//! executable is listed in `_CodeSignature/CodeResources` with Apple's rules for bundles
//! without a `Resources` directory (nested code included, as plain files), then the main
//! executable is signed with digests of `Info.plist` and that file.

use std::{
    cell::RefCell,
    collections::BTreeSet,
    fs, io,
    path::{Path, PathBuf},
    sync::atomic::{AtomicBool, Ordering},
};

use apple_codesign::{
    AppleCodesignError, BundleSigningContext, CodeResourcesBuilder, CodeResourcesRule,
    CodeSigningSlot, MachFile, MachOSigner, SettingsScope, SigningSettings,
    cryptography::DigestType, verify_macho, write_macho_file,
};
use plist::{Dictionary, Value};
use sha2::{Digest, Sha256};

use crate::{
    Error, MAX_PLIST_BYTES, Result,
    bundle::{self, CodeKind, short},
    check_plist,
    p12::SigningKey,
    package,
    sign::Toolchain,
};

pub(crate) struct Rcodesign<'a> {
    key: &'a SigningKey,
    /// Code signed so far, relative to the extracted root.
    signed: RefCell<Vec<PathBuf>>,
}

impl<'a> Rcodesign<'a> {
    pub(crate) fn new(key: &'a SigningKey) -> Self {
        Self {
            key,
            signed: RefCell::default(),
        }
    }

    /// The identity, its Apple certificate chain (when Apple issued it) and the team; no
    /// timestamp (`--timestamp=none`).
    fn settings(&self) -> SigningSettings<'a> {
        let mut settings = SigningSettings::default();
        settings.set_signing_key(&self.key.key, self.key.cert.clone());
        settings.chain_apple_certificates();
        settings.set_team_id(&self.key.team_id);
        settings
    }

    /// Files below `bundle` (relative to it) that earlier steps signed: the bundle seals them
    /// as they are instead of signing them again.
    fn signed_below(&self, root: &Path, bundle: &Path) -> io::Result<BTreeSet<PathBuf>> {
        let mut paths = BTreeSet::new();
        for done in self.signed.borrow().iter() {
            let Ok(rel) = done.strip_prefix(bundle) else {
                continue;
            };
            if rel.as_os_str().is_empty() {
                continue;
            }
            files_below(&root.join(done), rel, &mut paths)?;
        }
        Ok(paths)
    }

    fn sign_dylib(&self, path: &Path) -> Result<(), BundleError> {
        let (Some(dir), Some(name)) = (path.parent(), path.file_name()) else {
            return Err(BundleError::Invalid("invalid path".into()));
        };
        let settings = self.settings();
        let mut context = BundleSigningContext {
            settings: &settings,
            dest_dir: dir.to_path_buf(),
            previously_installed_paths: BTreeSet::new(),
            installed_paths: BTreeSet::new(),
        };
        context.sign_and_install_macho(path, Path::new(name))?;
        Ok(())
    }

    /// Seals and signs the bundle `dir` in place.
    fn sign_bundle(
        &self,
        dir: &Path,
        entitlements: Option<String>,
        signed_below: BTreeSet<PathBuf>,
    ) -> Result<(), BundleError> {
        let info_data = fs::read(dir.join("Info.plist"))
            .map_err(|e| BundleError::Invalid(format!("Info.plist: {e}")))?;
        let info =
            read_plist(&info_data).map_err(|e| BundleError::Invalid(format!("Info.plist {e}")))?;
        let executable = string(&info, "CFBundleExecutable")
            .filter(|name| !name.contains('/'))
            .ok_or_else(|| BundleError::Invalid("Info.plist names no executable".into()))?;
        let identifier = string(&info, "CFBundleIdentifier")
            .ok_or_else(|| BundleError::Invalid("Info.plist has no CFBundleIdentifier".into()))?;
        let exe_path = dir.join(executable);
        let exe_data =
            fs::read(&exe_path).map_err(|e| BundleError::Invalid(format!("{executable}: {e}")))?;

        // Like codesign: SHA-1 digests too when the executable targets an OS without SHA-256.
        let mut digests = vec![DigestType::Sha256];
        for macho in MachFile::parse(&exe_data)?.iter_macho() {
            let legacy = match macho.find_targeting()? {
                Some(target) => !target
                    .platform
                    .sha256_digest_support()?
                    .matches(&target.minimum_os_version),
                None => true,
            };
            if legacy {
                digests = vec![DigestType::Sha1, DigestType::Sha256];
                break;
            }
        }

        let mut settings = self.settings();
        let mut resources = CodeResourcesBuilder::default_no_resources_rules()?;
        resources.set_digests(digests.into_iter());
        for excluded in ["^_CodeSignature/", "^CodeResources$", "^_MASReceipt$"] {
            resources.add_exclusion_rule(CodeResourcesRule::new(excluded)?.exclude());
        }
        resources.add_exclusion_rule(
            CodeResourcesRule::new(format!("^{}$", regex::escape(executable)))?.exclude(),
        );
        let mut context = BundleSigningContext {
            settings: &settings,
            dest_dir: dir.to_path_buf(),
            previously_installed_paths: signed_below,
            installed_paths: BTreeSet::new(),
        };
        resources.walk_and_seal_directory(dir, dir, &mut context)?;
        let mut resources_data = Vec::new();
        resources.write_code_resources(&mut resources_data)?;
        let seal_dir = dir.join("_CodeSignature");
        fs::create_dir_all(&seal_dir)?;
        fs::write(seal_dir.join("CodeResources"), &resources_data)?;

        settings.set_binary_identifier(SettingsScope::Main, identifier);
        if let Some(xml) = entitlements {
            settings.set_entitlements_xml(SettingsScope::Main, xml)?;
        }
        settings.set_code_resources_data(SettingsScope::Main, resources_data);
        settings.set_info_plist_data(SettingsScope::Main, info_data);
        settings.import_settings_from_macho(&exe_data)?;
        let mut signed = Vec::with_capacity(exe_data.len() + (1 << 17));
        MachOSigner::new(&exe_data)?.write_signed_binary(&settings, &mut signed)?;
        write_macho_file(&exe_path, &exe_path, &signed)?;
        Ok(())
    }

    /// What `codesign --verify --strict` checks for one dylib, framework or app: valid
    /// signatures by this team on every architecture, and for bundles that `Info.plist` and
    /// every file match the seal and no file is left out.
    pub(crate) fn check(&self, path: &Path, kind: CodeKind) -> Result<(), String> {
        if kind == CodeKind::Dylib {
            let data = fs::read(path).map_err(|e| e.to_string())?;
            return self.check_binary(&data, None);
        }
        let info_data =
            fs::read(path.join("Info.plist")).map_err(|e| format!("Info.plist: {e}"))?;
        let info = read_plist(&info_data).map_err(|e| format!("Info.plist {e}"))?;
        let executable = string(&info, "CFBundleExecutable")
            .filter(|name| !name.contains('/'))
            .ok_or("Info.plist names no executable")?;
        let identifier =
            string(&info, "CFBundleIdentifier").ok_or("Info.plist has no CFBundleIdentifier")?;
        let resources = fs::read(path.join("_CodeSignature/CodeResources"))
            .map_err(|e| format!("_CodeSignature/CodeResources: {e}"))?;
        let data = fs::read(path.join(executable)).map_err(|e| format!("{executable}: {e}"))?;
        self.check_binary(
            &data,
            Some(Seal {
                identifier,
                info: &info_data,
                resources: &resources,
            }),
        )?;
        check_resources(path, &resources, executable)
    }

    fn check_binary(&self, data: &[u8], seal: Option<Seal<'_>>) -> Result<(), String> {
        let mach = MachFile::parse(data).map_err(|e| format!("not a Mach-O binary ({e})"))?;
        for macho in mach.iter_macho() {
            if let Some(problem) = verify_macho(macho).first() {
                return Err(problem.to_string());
            }
            let signature = macho
                .code_signature()
                .map_err(|e| e.to_string())?
                .ok_or("not signed")?;
            let directory = signature
                .code_directory()
                .map_err(|e| e.to_string())?
                .ok_or("no code directory")?;
            if directory.team_name.as_deref() != Some(self.key.team_id.as_str()) {
                return Err(format!(
                    "signed for team {} instead of {}",
                    directory.team_name.as_deref().unwrap_or("(none)"),
                    self.key.team_id
                ));
            }
            let Some(seal) = &seal else {
                continue;
            };
            if directory.ident != seal.identifier {
                return Err(format!(
                    "signed as {} instead of {}",
                    directory.ident, seal.identifier
                ));
            }
            for (slot, data, name) in [
                (CodeSigningSlot::Info, seal.info, "Info.plist"),
                (
                    CodeSigningSlot::ResourceDir,
                    seal.resources,
                    "_CodeSignature/CodeResources",
                ),
            ] {
                let expected = directory
                    .digest_type
                    .digest_data(data)
                    .map_err(|e| e.to_string())?;
                match directory.slot_digests().get(&slot) {
                    Some(digest) if *digest.data == *expected => {}
                    Some(_) => return Err(format!("{name} does not match the signature")),
                    None => return Err(format!("{name} is not sealed")),
                }
            }
        }
        Ok(())
    }
}

impl Toolchain for Rcodesign<'_> {
    fn sign(
        &self,
        root: &Path,
        path: &Path,
        entitlements: Option<&Path>,
        cancel: &AtomicBool,
    ) -> Result<()> {
        if cancel.load(Ordering::Relaxed) {
            return Err(Error::Cancelled);
        }
        let full = root.join(path);
        let failed = |e: &dyn std::fmt::Display| {
            Error::Tool(format!("Signing {} failed: {e}", path.display()))
        };
        let meta = fs::symlink_metadata(&full).map_err(Error::io_path("Could not read", &full))?;
        if meta.is_dir() {
            let entitlements = entitlements
                .map(|file| {
                    fs::read_to_string(file).map_err(Error::io_path("Could not read", file))
                })
                .transpose()?;
            let below = self
                .signed_below(root, path)
                .map_err(Error::io_path("Could not read", &full))?;
            self.sign_bundle(&full, entitlements, below)
                .map_err(|e| failed(&e))?;
        } else {
            self.sign_dylib(&full).map_err(|e| failed(&e))?;
        }
        self.signed.borrow_mut().push(path.to_path_buf());
        Ok(())
    }

    fn verify(&self, root: &Path, app: &Path, cancel: &AtomicBool) -> Result<()> {
        let app = root.join(app);
        let mut items: Vec<(PathBuf, CodeKind)> = bundle::nested_code(&app)?
            .into_iter()
            .map(|c| (c.path, c.kind))
            .collect();
        items.push((app, CodeKind::Bundle));
        for (path, kind) in items {
            if cancel.load(Ordering::Relaxed) {
                return Err(Error::Cancelled);
            }
            self.check(&path, kind).map_err(|problem| {
                Error::Tool(format!(
                    "Verifying the signature failed: {}: {problem}",
                    short(&path)
                ))
            })?;
        }
        Ok(())
    }

    fn package(&self, root: &Path, output: &Path, cancel: &AtomicBool) -> Result<()> {
        package::zip_payload(root, output, cancel)
    }
}

/// Why a bundle could not be signed.
#[derive(Debug, thiserror::Error)]
enum BundleError {
    #[error("{0}")]
    Invalid(String),
    #[error("{0}")]
    Io(#[from] io::Error),
    #[error("{0}")]
    Codesign(Box<AppleCodesignError>),
}

impl From<AppleCodesignError> for BundleError {
    fn from(e: AppleCodesignError) -> Self {
        Self::Codesign(Box::new(e))
    }
}

/// What a bundle's main executable must seal.
struct Seal<'a> {
    identifier: &'a str,
    info: &'a [u8],
    resources: &'a [u8],
}

fn read_plist(data: &[u8]) -> Result<Dictionary, String> {
    if data.len() as u64 > MAX_PLIST_BYTES {
        return Err("is too large".into());
    }
    check_plist(data).map_err(|e| format!("is not valid: {e}"))?;
    plist::from_bytes(data).map_err(|e| format!("is not valid: {e}"))
}

fn string<'a>(dict: &'a Dictionary, key: &str) -> Option<&'a str> {
    dict.get(key)
        .and_then(Value::as_string)
        .filter(|s| !s.is_empty())
}

/// Adds the files and symlinks at or below `path` to `out` as `rel/…` (links are not
/// followed).
fn files_below(path: &Path, rel: &Path, out: &mut BTreeSet<PathBuf>) -> io::Result<()> {
    if !fs::symlink_metadata(path)?.is_dir() {
        out.insert(rel.to_path_buf());
        return Ok(());
    }
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        files_below(&entry.path(), &rel.join(entry.file_name()), out)?;
    }
    Ok(())
}

/// Checks a bundle's `files2` seals against its files, both ways.
fn check_resources(dir: &Path, xml: &[u8], executable: &str) -> Result<(), String> {
    let resources = read_plist(xml).map_err(|e| format!("_CodeSignature/CodeResources {e}"))?;
    let files = resources
        .get("files2")
        .and_then(Value::as_dictionary)
        .ok_or("_CodeSignature/CodeResources has no files2")?;
    let mut nested = Vec::new();
    for (name, value) in files {
        let path = dir.join(name);
        let entry = value
            .as_dictionary()
            .ok_or_else(|| format!("{name}: invalid seal"))?;
        if fs::symlink_metadata(&path).is_err() {
            if entry.get("optional").and_then(Value::as_boolean) == Some(true) {
                continue;
            }
            return Err(format!("{name} is missing"));
        }
        if let Some(target) = entry.get("symlink").and_then(Value::as_string) {
            if fs::read_link(&path).ok().as_deref() != Some(Path::new(target)) {
                return Err(format!("{name} is not the sealed symbolic link"));
            }
        } else if entry.contains_key("cdhash") {
            nested.push(format!("{name}/"));
        } else {
            let sealed = entry
                .get("hash2")
                .and_then(Value::as_data)
                .ok_or_else(|| format!("{name} has no SHA-256 seal"))?;
            if sha256_file(&path)
                .map_err(|e| format!("{name}: {e}"))?
                .as_slice()
                != sealed
            {
                return Err(format!("{name} does not match the signature"));
            }
        }
    }
    let mut present = BTreeSet::new();
    files_below(dir, Path::new(""), &mut present).map_err(|e| e.to_string())?;
    for rel in present {
        let name = rel.to_string_lossy().replace('\\', "/");
        let unsealed = !files.contains_key(&name)
            && !omitted(&name, executable)
            && !nested.iter().any(|n| name.starts_with(n.as_str()));
        if unsealed {
            return Err(format!("{name} is not sealed"));
        }
    }
    Ok(())
}

fn sha256_file(path: &Path) -> io::Result<Vec<u8>> {
    use io::Read;
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = vec![0; 1 << 16];
    loop {
        match file.read(&mut buf)? {
            0 => return Ok(hasher.finalize().to_vec()),
            n => hasher.update(&buf[..n]),
        }
    }
}

/// Files the seal leaves out by design: the signature itself, the main executable and the
/// files Apple's rules omit.
fn omitted(name: &str, executable: &str) -> bool {
    let file_name = name.rsplit('/').next().unwrap_or(name);
    name.starts_with("_CodeSignature/")
        || name.starts_with("_MASReceipt/")
        || matches!(name, "Info.plist" | "PkgInfo" | "CodeResources")
        || name == executable
        || file_name == ".DS_Store"
        || (name.contains(".lproj/") && name.ends_with(".lproj/locversion.plist"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Identity, Profile, SignReport, SignRequest, SignStep, SigningDir,
        sign::sign_with,
        testutil::{
            ProfileSpec, TEAM, Zip, entitlements_xml, ios_macho, plist_bytes, profile_bytes,
            signing_identity,
        },
    };
    use chrono::{DateTime, TimeZone, Utc};
    use std::sync::Mutex;

    fn now() -> DateTime<Utc> {
        Utc.with_ymd_and_hms(2026, 11, 10, 12, 0, 0).unwrap()
    }

    fn info(entries: &[(&str, Value)]) -> Vec<u8> {
        plist_bytes(
            entries
                .iter()
                .map(|(k, v)| ((*k).to_owned(), v.clone()))
                .collect(),
            false,
        )
    }

    fn strings(items: &[&str]) -> Value {
        Value::Array(items.iter().map(|s| (*s).into()).collect())
    }

    /// An executable as a store or a previous signer left it: signed (ad hoc) with entitlements.
    fn presigned(entitlements: &[u8]) -> Vec<u8> {
        let mut settings = SigningSettings::default();
        settings.set_binary_identifier(SettingsScope::Main, "com.example.original");
        settings
            .set_entitlements_xml(
                SettingsScope::Main,
                std::str::from_utf8(entitlements).unwrap(),
            )
            .unwrap();
        let macho = ios_macho(false);
        let mut out = Vec::new();
        MachOSigner::new(&macho)
            .unwrap()
            .write_signed_binary(&settings, &mut out)
            .unwrap();
        out
    }

    fn demo_ipa() -> Vec<u8> {
        let main = presigned(&entitlements_xml(&[
            ("application-identifier", "OLDTEAM.com.example.demo".into()),
            (
                "com.apple.security.application-groups",
                strings(&["group.com.example.demo"]),
            ),
            ("aps-environment", "production".into()),
            ("com.apple.private.foo", true.into()),
        ]));
        let share = presigned(&entitlements_xml(&[(
            "com.apple.security.application-groups",
            strings(&["group.com.example.demo"]),
        )]));
        Zip::new()
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
            .file("Payload/Demo.app/Demo", &main, 0o755)
            .file("Payload/Demo.app/Assets.car", b"assets", 0o644)
            .file("Payload/Demo.app/Resources/data.txt", b"data", 0o644)
            .file("Payload/Demo.app/en.lproj/Localizable.strings", b"x", 0o644)
            .file("Payload/Demo.app/SC_Info/Demo.sinf", b"drm", 0o644)
            .file(
                "Payload/Demo.app/Frameworks/Lib.dylib",
                &ios_macho(true),
                0o755,
            )
            .file(
                "Payload/Demo.app/Frameworks/Kit.framework/Info.plist",
                &info(&[
                    ("CFBundleIdentifier", "com.example.Kit".into()),
                    ("CFBundleExecutable", "Kit".into()),
                    ("CFBundlePackageType", "FMWK".into()),
                ]),
                0o644,
            )
            .file(
                "Payload/Demo.app/Frameworks/Kit.framework/Kit",
                &ios_macho(true),
                0o755,
            )
            .symlink("Payload/Demo.app/Current", "Frameworks/Kit.framework")
            .file(
                "Payload/Demo.app/PlugIns/Share.appex/Info.plist",
                &info(&[
                    ("CFBundleIdentifier", "com.example.demo.Share".into()),
                    ("CFBundleExecutable", "Share".into()),
                ]),
                0o644,
            )
            .file("Payload/Demo.app/PlugIns/Share.appex/Share", &share, 0o755)
            .file("iTunesMetadata.plist", b"meta", 0o644)
            .finish()
    }

    struct Setup {
        dir: tempfile::TempDir,
        files: SigningDir,
        identity: Identity,
        certificate: Vec<u8>,
        req: SignRequest,
        explicit: Vec<u8>,
        wildcard: Vec<u8>,
    }

    fn setup(expires: &'static str) -> Setup {
        let dir = tempfile::tempdir().unwrap();
        let files = SigningDir::new(dir.path().join("signing"));
        let test = signing_identity(TEAM, "correct horse");
        let identity = files
            .set_identity(&test.p12, "correct horse", now())
            .unwrap();
        let bundle_id = format!("com.example.demo.{TEAM}");
        let wildcard = profile_bytes(&ProfileSpec {
            certificate: Some(test.certificate.clone()),
            expires,
            ..ProfileSpec::wildcard()
        });
        let mut explicit = ProfileSpec::explicit(&bundle_id);
        explicit.certificate = Some(test.certificate.clone());
        explicit.expires = expires;
        explicit.entitlements = vec![
            (
                "com.apple.security.application-groups",
                strings(&["group.com.example.kiwi"]),
            ),
            ("aps-environment", "development".into()),
        ];
        let explicit = profile_bytes(&explicit);
        files.add_profile(&wildcard, now()).unwrap();
        files.add_profile(&explicit, now()).unwrap();
        let profiles: Vec<Profile> = crate::load_profiles(&[files.profiles_dir()])
            .into_iter()
            .map(|(_, p)| p.unwrap())
            .collect();
        let input = dir.path().join("in.ipa");
        fs::write(&input, demo_ipa()).unwrap();
        let req = SignRequest {
            input_ipa: input,
            output_ipa: dir.path().join("out/Demo.ipa"),
            work_dir: dir.path().join("work"),
            identity: identity.clone(),
            profiles,
            url_scheme: Some(crate::url_scheme_for(&bundle_id)),
            bundle_id,
            strip_watch: true,
        };
        Setup {
            dir,
            files,
            identity,
            certificate: test.certificate,
            req,
            explicit,
            wildcard,
        }
    }

    fn sign(setup: &Setup) -> (SignReport, Vec<SignStep>) {
        let key = setup.files.signing_key().unwrap().unwrap();
        let steps = Mutex::new(Vec::new());
        let report = sign_with(
            &Rcodesign::new(&key),
            &setup.req,
            &AtomicBool::new(false),
            &|s| steps.lock().unwrap().push(s),
            now(),
        )
        .unwrap();
        (report, steps.into_inner().unwrap())
    }

    fn extract(ipa: &Path, dest: &Path) -> PathBuf {
        crate::archive::extract(ipa, dest, &AtomicBool::new(false)).unwrap()
    }

    /// The code directory of a signed binary: identifier, team and the entitlements slots.
    fn directory(path: &Path) -> (String, Option<String>, bool) {
        let data = fs::read(path).unwrap();
        let mach = MachFile::parse(&data).unwrap();
        let macho = mach.iter_macho().next().unwrap();
        let problems: Vec<String> = verify_macho(macho).iter().map(|p| p.to_string()).collect();
        assert!(problems.is_empty(), "{}: {problems:?}", path.display());
        let signature = macho.code_signature().unwrap().unwrap();
        let cd = signature.code_directory().unwrap().unwrap();
        let der = cd
            .slot_digests()
            .contains_key(&CodeSigningSlot::EntitlementsDer);
        (
            cd.ident.to_string(),
            cd.team_name.as_ref().map(|t| t.to_string()),
            der,
        )
    }

    #[test]
    fn signs_and_verifies_an_ipa_end_to_end() {
        let setup = setup("2027-10-26T18:52:06Z");
        let (report, steps) = sign(&setup);
        let id = format!("com.example.demo.{TEAM}");
        assert_eq!(
            steps,
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
        assert_eq!(report.bundle_id, id);
        assert_eq!(
            report.bundles,
            vec![
                ("com.example.demo".to_owned(), id.clone()),
                ("com.example.demo.Share".to_owned(), format!("{id}.Share")),
            ]
        );
        assert_eq!(
            report.profile_uuids,
            vec![format!("explicit-{id}"), ProfileSpec::wildcard().uuid]
        );
        assert_eq!(
            report.original_entitlements,
            [
                "application-identifier",
                "aps-environment",
                "com.apple.private.foo",
                "com.apple.security.application-groups"
            ]
        );
        assert_eq!(
            report.dropped_entitlements,
            [
                "com.apple.private.foo",
                "com.apple.security.application-groups"
            ]
        );
        assert_eq!(
            report.size,
            fs::metadata(&setup.req.output_ipa).unwrap().len()
        );

        let out = setup.dir.path().join("check");
        let app = extract(&setup.req.output_ipa, &out);
        // Every signature verifies, again from the packaged IPA.
        let key = setup.files.signing_key().unwrap().unwrap();
        let tools = Rcodesign::new(&key);
        tools
            .verify(&out, Path::new("Payload/Demo.app"), &AtomicBool::new(false))
            .unwrap();

        let (ident, team, der) = directory(&app.join("Demo"));
        assert_eq!(
            (ident.as_str(), team.as_deref(), der),
            (id.as_str(), Some(TEAM), true)
        );
        let (ident, _, _) = directory(&app.join("PlugIns/Share.appex/Share"));
        assert_eq!(ident, format!("{id}.Share"));
        let (ident, team, _) = directory(&app.join("Frameworks/Kit.framework/Kit"));
        assert_eq!(
            (ident.as_str(), team.as_deref()),
            ("com.example.Kit", Some(TEAM))
        );
        let (ident, _, _) = directory(&app.join("Frameworks/Lib.dylib"));
        assert_eq!(ident, "Lib");

        // The signature carries Kiwi's certificate.
        let main = fs::read(app.join("Demo")).unwrap();
        assert!(
            main.windows(setup.certificate.len())
                .any(|w| w == setup.certificate),
            "the certificate is embedded"
        );
        assert_eq!(
            setup.identity.sha1,
            crate::cert::sha1_hex(&setup.certificate)
        );

        // Entitlements: from each bundle's profile, limited to what the original asked for.
        let main_ent = crate::macho::entitlements(&app.join("Demo"))
            .unwrap()
            .unwrap();
        assert_eq!(
            main_ent.get("application-identifier"),
            Some(&format!("{TEAM}.{id}").into())
        );
        assert_eq!(
            main_ent.get("com.apple.developer.team-identifier"),
            Some(&TEAM.into())
        );
        assert_eq!(
            main_ent.get("com.apple.security.application-groups"),
            Some(&strings(&["group.com.example.kiwi"]))
        );
        assert_eq!(main_ent.get("aps-environment"), Some(&"development".into()));
        assert_eq!(main_ent.get("get-task-allow"), Some(&true.into()));
        assert!(!main_ent.contains_key("com.apple.private.foo"));
        let share_ent = crate::macho::entitlements(&app.join("PlugIns/Share.appex/Share"))
            .unwrap()
            .unwrap();
        assert_eq!(
            share_ent.get("application-identifier"),
            Some(&format!("{TEAM}.{id}.Share").into())
        );
        assert!(!share_ent.contains_key("com.apple.security.application-groups"));

        // Profiles, Info.plist changes, cleanup, links and modes survive packaging.
        assert_eq!(
            fs::read(app.join("embedded.mobileprovision")).unwrap(),
            setup.explicit
        );
        assert_eq!(
            fs::read(app.join("PlugIns/Share.appex/embedded.mobileprovision")).unwrap(),
            setup.wildcard
        );
        let main_info: Dictionary = plist::from_file(app.join("Info.plist")).unwrap();
        assert_eq!(
            main_info.get("CFBundleIdentifier"),
            Some(&id.clone().into())
        );
        assert!(main_info.contains_key("CFBundleURLTypes"));
        assert!(!app.join("SC_Info").exists());
        assert_eq!(
            fs::read_link(app.join("Current")).unwrap(),
            Path::new("Frameworks/Kit.framework")
        );
        let mut zip = zip::ZipArchive::new(fs::File::open(&setup.req.output_ipa).unwrap()).unwrap();
        let mut mode = |p: &str| zip.by_name(p).unwrap().unix_mode().unwrap() & 0o777;
        assert_eq!(mode("Payload/Demo.app/Demo"), 0o755);
        assert_eq!(mode("Payload/Demo.app/Assets.car"), 0o644);
        assert_eq!(mode("Payload/Demo.app/Frameworks/"), 0o755);

        // The top-level Resources folder is an ordinary sealed folder, as on iOS.
        let seal: Dictionary = plist::from_file(app.join("_CodeSignature/CodeResources")).unwrap();
        let files2 = seal.get("files2").and_then(Value::as_dictionary).unwrap();
        for sealed in [
            "Assets.car",
            "Resources/data.txt",
            "embedded.mobileprovision",
            "Frameworks/Kit.framework/Kit",
            "Frameworks/Kit.framework/_CodeSignature/CodeResources",
            "Frameworks/Lib.dylib",
            "PlugIns/Share.appex/Share",
            "Current",
        ] {
            assert!(files2.contains_key(sealed), "{sealed} is sealed");
        }
        assert!(!files2.contains_key("Demo") && !files2.contains_key("Info.plist"));
    }

    #[test]
    fn verification_catches_changes() {
        let setup = setup("2027-10-26T18:52:06Z");
        sign(&setup);
        let key = setup.files.signing_key().unwrap().unwrap();
        let tools = Rcodesign::new(&key);
        let verify = |root: &Path| {
            tools
                .verify(root, Path::new("Payload/Demo.app"), &AtomicBool::new(false))
                .map_err(|e| e.to_string())
        };
        let fresh = |name: &str| {
            let root = setup.dir.path().join(name);
            extract(&setup.req.output_ipa, &root);
            root
        };

        let root = fresh("changed");
        fs::write(root.join("Payload/Demo.app/Assets.car"), b"other").unwrap();
        let err = verify(&root).unwrap_err();
        assert_eq!(
            err,
            "Verifying the signature failed: Payload/Demo.app: Assets.car does not match the signature"
        );

        let root = fresh("added");
        fs::write(
            root.join("Payload/Demo.app/PlugIns/Share.appex/extra"),
            b"x",
        )
        .unwrap();
        let err = verify(&root).unwrap_err();
        assert!(
            err.ends_with("Payload/Demo.app/PlugIns/Share.appex: extra is not sealed"),
            "{err}"
        );

        let root = fresh("info");
        let info = root.join("Payload/Demo.app/Frameworks/Kit.framework/Info.plist");
        let mut bytes = fs::read(&info).unwrap();
        bytes.extend_from_slice(b"\n");
        fs::write(&info, bytes).unwrap();
        let err = verify(&root).unwrap_err();
        assert!(
            err.ends_with("Info.plist does not match the signature"),
            "{err}"
        );

        let root = fresh("binary");
        let lib = root.join("Payload/Demo.app/Frameworks/Lib.dylib");
        let mut bytes = fs::read(&lib).unwrap();
        bytes[0x4000] ^= 1;
        fs::write(&lib, bytes).unwrap();
        let err = verify(&root).unwrap_err();
        assert!(
            err.contains("Frameworks/Lib.dylib: code digest mismatch"),
            "{err}"
        );
    }

    #[test]
    fn signs_through_the_public_entry_point() {
        // Uses the real clock, so the profiles must outlive it.
        let setup = setup("2099-01-01T00:00:00Z");
        let report =
            crate::sign_ipa_with_files(&setup.req, &setup.files, &AtomicBool::new(false), &|_| {})
                .unwrap();
        assert_eq!(report.profile_uuids.len(), 2);
        assert!(setup.req.output_ipa.is_file());

        // A request planned for another identity is refused.
        let mut other = setup.req.clone();
        other.identity.sha1 = "0".repeat(40);
        let err =
            crate::sign_ipa_with_files(&other, &setup.files, &AtomicBool::new(false), &|_| {})
                .unwrap_err();
        assert!(err.to_string().contains("changed"), "{err}");

        // Without an uploaded identity there is nothing to sign with.
        setup.files.remove_identity().unwrap();
        let err =
            crate::sign_ipa_with_files(&setup.req, &setup.files, &AtomicBool::new(false), &|_| {})
                .unwrap_err();
        assert_eq!(err.to_string(), "No signing identity was uploaded");
    }

    #[test]
    fn cancels_while_signing() {
        let setup = setup("2027-10-26T18:52:06Z");
        let key = setup.files.signing_key().unwrap().unwrap();
        let cancel = AtomicBool::new(false);
        let progress = |step| {
            if step == (SignStep::Signing { done: 2, total: 4 }) {
                cancel.store(true, Ordering::Relaxed);
            }
        };
        let err =
            sign_with(&Rcodesign::new(&key), &setup.req, &cancel, &progress, now()).unwrap_err();
        assert!(matches!(err, Error::Cancelled));
        assert!(!setup.req.output_ipa.exists());
        assert_eq!(fs::read_dir(&setup.req.work_dir).unwrap().count(), 0);
    }

    #[test]
    fn omits_what_apple_omits() {
        assert!(omitted("Info.plist", "Demo"));
        assert!(omitted("Demo", "Demo"));
        assert!(omitted("_CodeSignature/CodeResources", "Demo"));
        assert!(omitted("Base.lproj/.DS_Store", "Demo"));
        assert!(omitted("en.lproj/locversion.plist", "Demo"));
        assert!(!omitted("Frameworks/Kit.framework/Info.plist", "Demo"));
        assert!(!omitted("embedded.mobileprovision", "Demo"));
        assert!(!omitted("Assets.car", "Demo"));
    }
}
