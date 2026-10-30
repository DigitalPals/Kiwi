//! End-to-end signing with the real keychain and Xcode profiles. Needs macOS and a GUI
//! (Aqua) session for codesign. Example:
//!
//! ```sh
//! KIWI_SIGN_IPA=~/app.ipa KIWI_SIGN_OUT=~/out KIWI_SIGN_IDENTITY=<sha1> \
//! KIWI_SIGN_BUNDLE_ID=com.example.app.TEAMID \
//!   cargo test -p kiwi-signer --test real_sign -- --ignored --nocapture
//! ```

use std::{path::PathBuf, sync::atomic::AtomicBool, time::Instant};

use kiwi_signer::{
    SignRequest, default_profile_dirs, list_identities, load_profiles, sign_ipa, url_scheme_for,
};

fn var(name: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| panic!("set {name}"))
}

#[test]
#[ignore = "needs macOS, a signing identity and KIWI_SIGN_* variables"]
fn signs_real_ipa() {
    let out = PathBuf::from(var("KIWI_SIGN_OUT"));
    let sha1 = var("KIWI_SIGN_IDENTITY");
    let bundle_id = var("KIWI_SIGN_BUNDLE_ID");
    let identity = list_identities()
        .expect("list identities")
        .into_iter()
        .find(|i| i.sha1.eq_ignore_ascii_case(&sha1))
        .expect("identity not in the keychain");
    eprintln!("identity: {identity:?}");
    let profiles = load_profiles(&default_profile_dirs())
        .into_iter()
        .filter_map(|(path, result)| match result {
            Ok(profile) => {
                eprintln!(
                    "profile {} {} ({})",
                    profile.uuid, profile.app_id_pattern, profile.name
                );
                Some(profile)
            }
            Err(e) => {
                eprintln!("skipping {}: {e}", path.display());
                None
            }
        })
        .collect();
    let req = SignRequest {
        input_ipa: PathBuf::from(var("KIWI_SIGN_IPA")),
        output_ipa: out.join("signed.ipa"),
        work_dir: out.join("work"),
        identity,
        profiles,
        url_scheme: Some(url_scheme_for(&bundle_id)),
        bundle_id: bundle_id.clone(),
        strip_watch: true,
        devices: Vec::new(),
    };
    let started = Instant::now();
    let report = sign_ipa(&req, &AtomicBool::new(false), &|step| {
        eprintln!("[{:6.2}s] {step}", started.elapsed().as_secs_f64());
    })
    .expect("signing failed");
    eprintln!(
        "signed in {:.2}s: {report:#?}",
        started.elapsed().as_secs_f64()
    );
    assert_eq!(report.bundle_id, bundle_id);
    assert_eq!(
        report.size,
        std::fs::metadata(&req.output_ipa).unwrap().len()
    );
}
