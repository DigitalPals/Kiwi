use std::ffi::OsStr;
use std::path::Path;
use std::process::{Output, Stdio};
use std::time::Duration;

use crate::{Error, Result};

/// Runs `program` to completion, killing it when `timeout` elapses.
pub(crate) async fn run<S: AsRef<OsStr>>(
    program: &Path,
    args: &[S],
    timeout: Duration,
) -> Result<Output> {
    let child = tokio::process::Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .output();
    match tokio::time::timeout(timeout, child).await {
        Ok(Ok(output)) => Ok(output),
        Ok(Err(source)) => Err(Error::Spawn {
            program: name(program),
            source,
        }),
        Err(_) => Err(Error::Timeout {
            program: name(program),
            after: timeout,
        }),
    }
}

pub(crate) fn name(program: &Path) -> String {
    program
        .file_name()
        .unwrap_or(program.as_os_str())
        .to_string_lossy()
        .into_owned()
}

/// A short, single-line description of a failed process for error messages.
pub(crate) fn failure_message(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stderr = stderr.trim();
    let first = stderr.lines().next().unwrap_or_default().trim();
    if first.is_empty() {
        format!("exited with {}", output.status)
    } else {
        first.chars().take(500).collect()
    }
}
