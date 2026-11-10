//! The macOS backend's tools: Apple's `codesign` (with the keychain) and `/usr/bin/zip`.

use std::{path::Path, process::Command, sync::atomic::AtomicBool, time::Duration};

use crate::{Error, Result, process, sign::Toolchain};

/// Longest one `codesign` call may run. It blocks while macOS waits for an answer to a
/// keychain prompt, which would otherwise hold the signing slot forever.
const CODESIGN_TIMEOUT: Duration = Duration::from_secs(10 * 60);
/// Longest zipping the signed app may run (large apps on a busy disk).
const ZIP_TIMEOUT: Duration = Duration::from_secs(30 * 60);

pub(crate) struct AppleTools<'a> {
    pub identity: &'a str,
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

#[cfg(test)]
mod tests {
    use super::*;

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
}
