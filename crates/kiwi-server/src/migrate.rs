//! `kiwi migrate import`: the last step of moving Kiwi from a Mac to Linux (migrate.sh,
//! docs/migrate.md).
//!
//! `migrate.sh export` packs the Mac's data directory with a copy of its database, the
//! signing identity from the login keychain (as a `.p12`), the provisioning profiles Xcode
//! keeps and, when the Mac stays on as the Mac builder, a pairing made ahead of time.
//! `migrate.sh import` puts the data directory in place with Kiwi stopped and runs this, as
//! Kiwi's user, to adjust what still names the Mac: the paths of signed apps in the
//! database, the paired Mac builder, and the identity and profiles Linux signs with.
use std::{
    fs,
    path::{Path, PathBuf},
};

use chrono::{DateTime, Utc};
use serde::Deserialize;
use serde_json::Value;

use crate::{db::Db, mac_builder};

/// The bundle format this Kiwi reads (`manifest.json` `format`).
pub const FORMAT: u32 = 1;

/// `kiwi migrate …`
#[derive(Debug, clap::Subcommand)]
pub enum Command {
    /// Finish importing a Mac's Kiwi: run by `migrate.sh import` with Kiwi stopped, after it
    /// put the Mac's data directory in place
    Import(ImportArgs),
}

#[derive(Debug, clap::Args)]
pub struct ImportArgs {
    /// Kiwi's data directory, with the Mac's files in it
    #[arg(long)]
    pub data_dir: PathBuf,
    /// The unpacked bundle (manifest.json, signing/, mac-builder.json)
    #[arg(long)]
    pub bundle: PathBuf,
}

#[derive(Debug, Deserialize)]
struct Manifest {
    format: u32,
    source: Source,
}

#[derive(Debug, Deserialize)]
struct Source {
    /// The data directory on the Mac.
    data_dir: String,
}

/// `kiwi migrate …`.
pub fn main(command: Command) -> Result<(), String> {
    match command {
        Command::Import(args) => {
            for line in import(&args.data_dir, &args.bundle, Utc::now())? {
                println!("{line}");
            }
            Ok(())
        }
    }
}

/// Adjusts the data directory `data` to this computer; returns what it did, a line each.
pub fn import(data: &Path, bundle: &Path, now: DateTime<Utc>) -> Result<Vec<String>, String> {
    let manifest: Manifest = read_json(&bundle.join("manifest.json"))?;
    if manifest.format != FORMAT {
        return Err(format!(
            "The bundle has format {}, this Kiwi reads format {FORMAT}: use the Kiwi that made it",
            manifest.format
        ));
    }
    let db_path = data.join("kiwi.db");
    if !db_path.is_file() {
        return Err(format!("{} is missing", db_path.display()));
    }
    // Opening it also brings the schema up to this Kiwi's.
    let db = Db::open(&db_path)?;
    let mut report = Vec::new();

    let old = with_slash(&manifest.source.data_dir);
    let new = with_slash(&data.to_string_lossy());
    let moved = db.move_build_files(&old, &new).map_err(|e| e.to_string())?;
    let missing = db
        .builds()
        .map_err(|e| e.to_string())?
        .iter()
        .filter_map(|b| b.ipa_path.as_deref())
        .filter(|path| !Path::new(path).is_file())
        .count();
    report.push(format!("Signed apps: {moved} moved to {}", data.display()));
    if missing > 0 {
        report.push(format!("Signed apps: the files of {missing} are missing"));
    }

    let builder = bundle.join("mac-builder.json");
    if builder.is_file() {
        let record: Value = read_json(&builder)?;
        let name = mac_builder::import_record(&db, &record)?;
        report.push(format!("Mac builder: paired with {name}"));
    }
    drop(db);

    report.extend(import_signing(
        &crate::signing_dir(data),
        &bundle.join("signing"),
        now,
    )?);
    Ok(report)
}

/// Stores `from/identity.p12` (with `identity.password`) and `from/profiles/*.mobileprovision`
/// as uploads, checked as an upload is. A profile Kiwi refuses (an expired one) is skipped,
/// and so is one that doesn't allow the identity (Xcode also keeps App Store and Ad Hoc
/// profiles, which Kiwi can't sign with).
fn import_signing(
    dir: &kiwi_signer::SigningDir,
    from: &Path,
    now: DateTime<Utc>,
) -> Result<Vec<String>, String> {
    let mut report = Vec::new();
    let mut certificate = None;
    let p12 = from.join("identity.p12");
    if p12.is_file() {
        let bytes = zeroize::Zeroizing::new(read(&p12)?);
        let password = zeroize::Zeroizing::new(
            String::from_utf8(read(&from.join("identity.password"))?)
                .map_err(|_| "identity.password is not text".to_owned())?,
        );
        let identity = dir
            .set_identity(&bytes, password.trim_end_matches(['\r', '\n']), now)
            .map_err(|e| format!("The signing identity: {e}"))?;
        report.push(format!("Signing identity: {}", identity.name));
        certificate = Some(identity.sha1);
    }
    let mut profiles: Vec<PathBuf> = match fs::read_dir(from.join("profiles")) {
        Ok(entries) => entries
            .filter_map(|e| e.ok().map(|e| e.path()))
            .filter(|p| p.extension().is_some_and(|e| e == "mobileprovision"))
            .collect(),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Vec::new(),
        Err(e) => return Err(format!("{}: {e}", from.join("profiles").display())),
    };
    profiles.sort();
    for path in profiles {
        let file = path.file_name().unwrap_or_default().to_string_lossy();
        let bytes = read(&path)?;
        if let (Some(sha1), Ok(profile)) = (&certificate, kiwi_signer::parse_profile(&bytes))
            && !profile
                .certificate_sha1s
                .iter()
                .any(|c| c.eq_ignore_ascii_case(sha1))
        {
            report.push(format!(
                "Profile skipped ({file}): '{}' is for another certificate",
                profile.name
            ));
            continue;
        }
        match dir.add_profile(&bytes, now) {
            Ok(profile) => report.push(format!("Profile: {}", profile.name)),
            Err(e) => report.push(format!("Profile skipped ({file}): {e}")),
        }
    }
    Ok(report)
}

fn with_slash(path: &str) -> String {
    format!("{}/", path.trim_end_matches('/'))
}

fn read(path: &Path) -> Result<Vec<u8>, String> {
    fs::read(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T, String> {
    serde_json::from_slice(&read(path)?).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mac_builder::Hello;

    fn sha(c: char) -> String {
        c.to_string().repeat(64)
    }

    const RELEASE: &str = r#"{"version":"1.0","download_url":"https://example.com/a.ipa"}"#;

    fn hello() -> Hello {
        Hello {
            version: "0.2.0".into(),
            os: "macOS 27.0 (arm64)".into(),
            runner_sha256: sha('a'),
            recipes_sha256: sha('b'),
        }
    }

    /// A data directory as migrate.sh leaves it: the Mac's database, with signed apps under
    /// the Mac's data directory, one of them present here.
    fn data_dir(root: &Path) -> PathBuf {
        let data = root.join("data");
        fs::create_dir_all(data.join("builds/b1")).unwrap();
        fs::write(data.join("builds/b1/App.ipa"), b"ipa").unwrap();
        drop(Db::open(&data.join("kiwi.db")).unwrap());
        let conn = rusqlite::Connection::open(data.join("kiwi.db")).unwrap();
        let mac = "/Users/me/Library/Application Support/Kiwi";
        for (id, path) in [
            ("b1", format!("{mac}/builds/b1/App.ipa")),
            ("b2", format!("{mac}/builds/b2/Gone.ipa")),
            ("b3", "/elsewhere/Other.ipa".to_owned()),
        ] {
            conn.execute(
                "INSERT INTO builds (id, source_id, bundle_id, app_name, version, release,
                     signed_bundle_id, status, ipa_path, created_at)
                 VALUES (?1, 's', 'com.example.app', 'Example', '1.0', ?3,
                     'com.example.app.T', 'ready', ?2, '2026-11-01T00:00:00Z')",
                [id, path.as_str(), RELEASE],
            )
            .unwrap();
        }
        data
    }

    fn bundle(root: &Path, format: u32) -> PathBuf {
        let bundle = root.join("bundle");
        fs::create_dir_all(&bundle).unwrap();
        fs::write(
            bundle.join("manifest.json"),
            serde_json::to_vec(&serde_json::json!({
                "format": format,
                "source": {"data_dir": "/Users/me/Library/Application Support/Kiwi/"},
                "public_url": "https://kiwi.example.ts.net",
            }))
            .unwrap(),
        )
        .unwrap();
        bundle
    }

    #[test]
    fn moves_signed_apps_and_pairs_the_builder() {
        let root = tempfile::tempdir().unwrap();
        let data = data_dir(root.path());
        let bundle = bundle(root.path(), FORMAT);
        let (token, record) = mac_builder::provision("Mac Studio", hello()).unwrap();
        assert!(token.len() >= 43);
        fs::write(
            bundle.join("mac-builder.json"),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();

        let report = import(&data, &bundle, Utc::now()).unwrap();
        assert!(report.contains(&format!("Signed apps: 2 moved to {}", data.display())));
        assert!(
            report
                .iter()
                .any(|l| l == "Signed apps: the files of 2 are missing")
        );
        assert!(report.contains(&"Mac builder: paired with Mac Studio".to_owned()));

        let db = Db::open(&data.join("kiwi.db")).unwrap();
        let path = |id: &str| db.build(id).unwrap().unwrap().ipa_path.unwrap();
        assert_eq!(path("b1"), format!("{}/builds/b1/App.ipa", data.display()));
        assert_eq!(path("b2"), format!("{}/builds/b2/Gone.ipa", data.display()));
        assert_eq!(path("b3"), "/elsewhere/Other.ipa");
        let saved: Value = serde_json::from_str(&db.meta("mac_builder").unwrap().unwrap()).unwrap();
        assert_eq!(saved["name"], "Mac Studio");
        assert_eq!(saved["token_sha256"], record["token_sha256"]);
        assert!(saved["last_seen"].is_null());
        // The builder's token is what Kiwi checks it against.
        assert_eq!(
            hex::encode(<sha2::Sha256 as sha2::Digest>::digest(token.as_bytes())),
            saved["token_sha256"].as_str().unwrap()
        );
        assert!(mac_builder::MacBuilder::load(&db).is_ok());
    }

    #[test]
    fn refuses_other_formats_and_bad_pairings() {
        let root = tempfile::tempdir().unwrap();
        let data = data_dir(root.path());
        let bundle = bundle(root.path(), FORMAT + 1);
        let error = import(&data, &bundle, Utc::now()).unwrap_err();
        assert!(error.contains("format 2"), "{error}");

        let bundle = self::bundle(root.path(), FORMAT);
        let (_, mut record) = mac_builder::provision("Mac", hello()).unwrap();
        record["token_sha256"] = "short".into();
        fs::write(
            bundle.join("mac-builder.json"),
            serde_json::to_vec(&record).unwrap(),
        )
        .unwrap();
        let error = import(&data, &bundle, Utc::now()).unwrap_err();
        assert!(error.contains("Mac builder"), "{error}");
        assert!(mac_builder::provision(" ", hello()).is_err());
    }

    #[test]
    fn provisioning_saves_the_builders_side_once() {
        let root = tempfile::tempdir().unwrap();
        let builder = root.path().join("builder");
        let provision = |replace| {
            mac_builder::client::provision(
                &builder,
                "https://kiwi.example.ts.net/",
                Some("Mac Studio".into()),
                replace,
            )
        };
        let record = provision(false).unwrap();
        let saved = mac_builder::client::load(&builder).unwrap();
        assert_eq!(saved.server, "https://kiwi.example.ts.net");
        assert_eq!(saved.name, "Mac Studio");
        assert_eq!(
            hex::encode(<sha2::Sha256 as sha2::Digest>::digest(
                saved.token.as_bytes()
            )),
            record["token_sha256"].as_str().unwrap()
        );
        assert_eq!(record["version"], env!("CARGO_PKG_VERSION"));
        let error = provision(false).unwrap_err();
        assert!(error.contains("--replace"), "{error}");
        let again = provision(true).unwrap();
        assert_ne!(again["token_sha256"], record["token_sha256"]);
        assert!(
            mac_builder::client::provision(&builder, "http://kiwi.example.ts.net", None, true)
                .is_err()
        );
    }

    #[test]
    fn needs_the_database() {
        let root = tempfile::tempdir().unwrap();
        let bundle = bundle(root.path(), FORMAT);
        let error = import(&root.path().join("empty"), &bundle, Utc::now()).unwrap_err();
        assert!(error.contains("kiwi.db is missing"), "{error}");
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn stores_the_identity_and_the_profiles_that_are_still_valid() {
        use kiwi_signer::testutil::{ProfileSpec, TEAM, profile_bytes, signing_identity};
        let root = tempfile::tempdir().unwrap();
        let data = data_dir(root.path());
        let bundle = bundle(root.path(), FORMAT);
        let signing = bundle.join("signing");
        fs::create_dir_all(signing.join("profiles")).unwrap();
        let identity = signing_identity(TEAM, "secret pass");
        fs::write(signing.join("identity.p12"), &identity.p12).unwrap();
        fs::write(signing.join("identity.password"), "secret pass\n").unwrap();
        let mut valid = ProfileSpec::wildcard();
        valid.certificate = Some(identity.certificate.clone());
        valid.expires = "2099-01-01T00:00:00Z";
        fs::write(
            signing.join("profiles/valid.mobileprovision"),
            profile_bytes(&valid),
        )
        .unwrap();
        let mut expired = ProfileSpec::wildcard();
        expired.uuid = "11111111-2222-3333-4444-555555555555".into();
        expired.expires = "2020-01-01T00:00:00Z";
        fs::write(
            signing.join("profiles/expired.mobileprovision"),
            profile_bytes(&expired),
        )
        .unwrap();
        let mut other = ProfileSpec::wildcard();
        other.uuid = "66666666-7777-8888-9999-000000000000".into();
        other.name = "App Store profile".into();
        other.expires = "2099-01-01T00:00:00Z";
        fs::write(
            signing.join("profiles/other.mobileprovision"),
            profile_bytes(&other),
        )
        .unwrap();
        fs::write(signing.join("profiles/notes.txt"), "not a profile").unwrap();

        let report = import(&data, &bundle, Utc::now()).unwrap();
        assert!(
            report.contains(&format!(
                "Signing identity: Apple Development: Kiwi Test ({TEAM})"
            )),
            "{report:?}"
        );
        assert!(
            report.contains(&format!("Profile: {}", valid.name)),
            "{report:?}"
        );
        assert!(
            report
                .iter()
                .any(|l| l.starts_with("Profile skipped (expired.mobileprovision)")),
            "{report:?}"
        );
        assert!(
            report.contains(
                &"Profile skipped (other.mobileprovision): 'App Store profile' is for another certificate"
                    .to_owned()
            ),
            "{report:?}"
        );
        let dir = crate::signing_dir(&data);
        assert_eq!(
            dir.identity().unwrap().unwrap().team_id.as_deref(),
            Some(TEAM)
        );
        let stored: Vec<_> = fs::read_dir(dir.profiles_dir()).unwrap().collect();
        assert_eq!(stored.len(), 1);

        // A wrong password stops the import with the reason.
        fs::write(signing.join("identity.password"), "wrong").unwrap();
        let error = import(&data, &bundle, Utc::now()).unwrap_err();
        assert!(error.starts_with("The signing identity:"), "{error}");
    }
}
