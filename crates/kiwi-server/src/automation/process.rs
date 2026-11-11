//! Running a Kiwi Builds runner and reading its events, shared by Kiwi's own builds
//! ([`super::invoke`]) and by the Mac builder ([`crate::mac_builder`]): Kiwi validates and
//! applies the events ([`Events`]), wherever the runner ran; the Mac builder runs the
//! process ([`run`]) and forwards them.
use std::{path::Path, process::Stdio, time::Duration};

use kiwi_source::CancellationToken;
use serde_json::Value;
use tokio::io::{AsyncBufReadExt, AsyncReadExt, BufReader};

use super::{Failure, failed};
use crate::Kiwi;

/// The most a runner may write to stdout (its events) in one run.
pub const EVENT_LIMIT: usize = 2 * 1024 * 1024;
/// The longest build stage Kiwi shows.
pub const STAGE_LIMIT: usize = 256;

/// One run of a runner script: `python program command --data-dir … --request …
/// --job-id id [--recipe recipe] --supervisor-pid <this process>`.
pub struct Run<'a> {
    pub python: &'a Path,
    pub program: &'a Path,
    pub command: &'a str,
    pub data_dir: &'a Path,
    pub request: &'a Path,
    pub id: &'a str,
    pub recipe: Option<&'a str>,
    /// How messages name the builder: "The Android builder".
    pub builder: &'a str,
    pub timeout: Duration,
}

/// Runs the runner in a process group of its own and hands each line it prints, parsed as
/// JSON, to `on_event`. Returns whether it exited successfully. When `on_event` fails, the
/// token is cancelled or the time limit passes, the runner's process group is stopped
/// ([`stop`]) and the error returned.
pub async fn run(
    call: &Run<'_>,
    token: &CancellationToken,
    mut on_event: impl FnMut(Value) -> Result<(), Failure>,
) -> Result<bool, Failure> {
    let builder = call.builder;
    let mut cmd = tokio::process::Command::new(call.python);
    cmd.arg(call.program)
        .arg(call.command)
        .arg("--data-dir")
        .arg(call.data_dir)
        .arg("--request")
        .arg(call.request)
        .arg("--job-id")
        .arg(call.id);
    if let Some(recipe) = call.recipe {
        cmd.arg("--recipe").arg(recipe);
    }
    cmd.arg("--supervisor-pid")
        .arg(std::process::id().to_string())
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        cmd.as_std_mut().process_group(0);
    }
    let mut child = cmd.spawn().map_err(|e| {
        (
            format!(
                "{builder} could not start Python ({}): {e}. Install Python 3.12 or newer, \
                 or name it with --python (KIWI_PYTHON).",
                call.python.display()
            ),
            true,
        )
    })?;
    let mut lines = BufReader::new(child.stdout.take().unwrap().take(EVENT_LIMIT as u64)).lines();
    let deadline = tokio::time::sleep(call.timeout);
    tokio::pin!(deadline);
    let read = async {
        while let Some(line) = lines.next_line().await.map_err(failed)? {
            on_event(serde_json::from_str(&line).map_err(failed)?)?;
        }
        Ok(child.wait().await.map_err(failed)?.success())
    };
    let result = tokio::select! {
        result = read => result,
        _ = token.cancelled() => Err(failed("Cancelled")),
        _ = &mut deadline => Err(failed(format!("{builder} exceeded its time limit"))),
    };
    if result.is_err() {
        stop(&mut child).await;
    }
    result
}

/// Terminate the runner and all commands it supervises. The Python SIGTERM handler
/// reaps its own process groups; escalation also bounds a broken runner's shutdown.
#[cfg(unix)]
pub async fn stop(child: &mut tokio::process::Child) {
    if let Some(pid) = child.id() {
        unsafe {
            libc::kill(-(pid as i32), libc::SIGTERM);
        }
        if tokio::time::timeout(Duration::from_secs(5), child.wait())
            .await
            .is_err()
        {
            unsafe {
                libc::kill(-(pid as i32), libc::SIGKILL);
            }
            let _ = child.wait().await;
        }
    }
}
#[cfg(not(unix))]
pub async fn stop(child: &mut tokio::process::Child) {
    let _ = child.kill().await;
}

/// What a runner reported, checked as it arrives: progress (`stage`), the setup checks
/// (`health`, saved in the automation state), `community` and `error` events (also given to
/// the caller), and the results (`plan`, `artifact`, `kiwi_app`). Anything else fails the
/// run.
#[derive(Default)]
pub struct Events {
    /// `plan`, `artifact` and `kiwi_app` events, in order.
    pub results: Vec<Value>,
    runner_error: Option<Failure>,
}

impl Events {
    pub fn apply(
        &mut self,
        kiwi: &Kiwi,
        event: Value,
        on_stage: &mut dyn FnMut(&str) -> Result<(), Failure>,
        on_event: &mut dyn FnMut(&Value) -> Result<(), Failure>,
    ) -> Result<(), Failure> {
        match event["event"].as_str() {
            Some("stage") => {
                let stage = event["stage"]
                    .as_str()
                    .ok_or_else(|| failed("Invalid build stage"))?;
                if stage.len() > STAGE_LIMIT {
                    return Err(failed("Build stage exceeds its limit"));
                }
                on_stage(stage)?;
            }
            Some("health") => {
                let checks = event["checks"]
                    .as_array()
                    .ok_or_else(|| failed("Invalid setup checks"))?
                    .clone();
                kiwi.automation
                    .change(&kiwi.db, |s| {
                        s.checks = checks;
                        Ok(())
                    })
                    .map_err(failed)?;
            }
            Some("community") => on_event(&event)?,
            Some("error") => {
                let _ = on_event(&event);
                self.runner_error = Some((
                    event["message"]
                        .as_str()
                        .unwrap_or("The builder failed")
                        .chars()
                        .take(1024)
                        .collect(),
                    event["blocked"].as_bool().unwrap_or(false),
                ))
            }
            Some("plan" | "artifact" | "kiwi_app") => self.results.push(event),
            _ => return Err(failed("Unknown builder event")),
        }
        Ok(())
    }

    /// The error the runner reported, if any.
    pub fn runner_error(&self) -> Option<&Failure> {
        self.runner_error.as_ref()
    }

    /// The results of a run that ended: the runner's own error first, then whether it
    /// exited successfully.
    pub fn finish(self, success: bool, builder: &str) -> Result<Vec<Value>, Failure> {
        if let Some(error) = self.runner_error {
            return Err(error);
        }
        if !success {
            return Err(failed(format!(
                "{builder} stopped unexpectedly. Check its private log."
            )));
        }
        Ok(self.results)
    }
}
