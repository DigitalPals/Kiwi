//! Runs Apple's command-line tools with a deadline.

use std::{
    io::Read,
    path::Path,
    process::{Command, Output, Stdio},
    sync::atomic::{AtomicBool, Ordering},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use crate::{Error, Result};

/// Runs `cmd` to completion and collects its output. The tool is killed when `cancel` is
/// set ([`Error::Cancelled`]) or when it runs longer than `timeout`: `codesign` and
/// `security` block indefinitely while macOS waits for an answer to a keychain prompt.
pub(crate) fn output(
    cmd: &mut Command,
    what: &str,
    timeout: Duration,
    cancel: &AtomicBool,
) -> Result<Output> {
    let mut child = cmd
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| Error::Tool(format!("{what} failed: {e}")))?;
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let deadline = Instant::now() + timeout;
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if cancel.load(Ordering::Relaxed) => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Error::Cancelled);
            }
            Ok(None) if Instant::now() >= deadline => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(Error::Tool(timeout_message(
                    what,
                    timeout,
                    Path::new(cmd.get_program()),
                )));
            }
            Ok(None) => thread::sleep(Duration::from_millis(20)),
            Err(e) => return Err(Error::Tool(format!("{what} failed: {e}"))),
        }
    };
    Ok(Output {
        status,
        stdout: stdout.join().unwrap_or_default(),
        stderr: stderr.join().unwrap_or_default(),
    })
}

/// Reads a pipe to the end on its own thread, so a chatty tool never blocks on a full pipe.
fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> JoinHandle<Vec<u8>> {
    thread::spawn(move || {
        let mut out = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut out);
        }
        out
    })
}

fn timeout_message(what: &str, timeout: Duration, program: &Path) -> String {
    let limit = match timeout.as_secs() {
        0 => format!("{} ms", timeout.as_millis()),
        s if s % 60 == 0 => format!("{} min", s / 60),
        s => format!("{s} s"),
    };
    let mut message = format!("{what} did not finish within {limit} and was stopped");
    let keychain = ["codesign", "security"]
        .iter()
        .any(|tool| program.file_name().is_some_and(|name| name == *tool));
    if keychain {
        message.push_str(
            ". If the Mac shows a keychain prompt, allow access to the signing key, then retry",
        );
    }
    message
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collects_output_and_stops_slow_tools() {
        let out = output(
            Command::new("/bin/sh").args(["-c", "echo out; echo err >&2; exit 2"]),
            "sh",
            Duration::from_secs(30),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(out.stdout, b"out\n");
        assert_eq!(out.stderr, b"err\n");
        assert_eq!(out.status.code(), Some(2));

        let started = Instant::now();
        let err = output(
            Command::new("/bin/sleep").arg("10"),
            "Sleeping",
            Duration::from_millis(200),
            &AtomicBool::new(false),
        )
        .unwrap_err();
        assert_eq!(
            err.to_string(),
            "Sleeping did not finish within 200 ms and was stopped"
        );
        assert!(started.elapsed() < Duration::from_secs(5));

        let err = output(
            Command::new("/bin/sleep").arg("10"),
            "sleep",
            Duration::from_secs(30),
            &AtomicBool::new(true),
        )
        .unwrap_err();
        assert!(matches!(err, Error::Cancelled));
    }

    #[test]
    fn timeouts_of_keychain_tools_mention_the_prompt() {
        let codesign = timeout_message(
            "Signing Payload/A.app",
            Duration::from_secs(600),
            Path::new("/usr/bin/codesign"),
        );
        assert_eq!(
            codesign,
            "Signing Payload/A.app did not finish within 10 min and was stopped. If the Mac \
             shows a keychain prompt, allow access to the signing key, then retry"
        );
        let zip = timeout_message(
            "Packaging the IPA",
            Duration::from_secs(1800),
            Path::new("/usr/bin/zip"),
        );
        assert_eq!(
            zip,
            "Packaging the IPA did not finish within 30 min and was stopped"
        );
        assert!(
            timeout_message(
                "security",
                Duration::from_secs(30),
                Path::new("/usr/bin/security")
            )
            .starts_with("security did not finish within 30 s and was stopped. If the Mac")
        );
    }
}
