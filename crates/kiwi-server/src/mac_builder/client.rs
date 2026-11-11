//! `kiwi builder pair|provision|run`: the Mac builder program, on the Mac (docs/kiwi-builds.md "Mac
//! builder").
//!
//! It connects out to a Kiwi (HTTPS with a verified certificate; plain HTTP only to a
//! loopback address), asks for work with a long poll and runs Kiwi Builds' iOS runner, the
//! runner.py and recipes.json embedded in this program, never code from Kiwi. It forwards
//! the runner's events as they arrive (and a heartbeat while it is quiet), uploads the
//! patched IPA of a build, reports how the run ended and then deletes the IPA. Kiwi
//! validates, signs and publishes it.
use std::{
    path::{Path, PathBuf},
    sync::RwLock,
    time::Duration,
};

use chrono::Utc;
use kiwi_source::CancellationToken;
use reqwest::{
    Method, StatusCode,
    header::{CONTENT_LENGTH, CONTENT_TYPE, HeaderMap},
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use tokio::sync::mpsc;

use super::Hello;
use crate::automation::{self, process, valid_sha};

/// The Python of the Mac setup (Homebrew's 3.13), as for Kiwi itself.
pub const PYTHON: &str = crate::config::PYTHON;
/// Where the toolchain lives (no spaces: Theos can't build in Application Support). Kiwi on
/// the same Mac shares it; job folders are per job id.
pub const TOOLS: &str = ".kiwi-builder-tools";
const SAVED: &str = "builder.json";
const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// How long a call may keep failing before the builder gives a task up (Kiwi drops it
/// after 120 s without a call anyway).
const GIVE_UP: Duration = Duration::from_secs(180);
/// The largest task Kiwi may send.
const TASK_LIMIT: usize = 1024 * 1024;

/// `kiwi builder …`
#[derive(Debug, clap::Subcommand)]
pub enum Command {
    /// Run the Mac builder: ask Kiwi for work and run Kiwi Builds' iOS runner for it.
    Run(RunArgs),
    /// Pair this Mac with a Kiwi, with the one-time code from its Settings › Mac builder.
    Pair(PairArgs),
    /// Pair this Mac ahead of time with a Kiwi that doesn't run yet (migrate.sh export):
    /// saves the pairing here and prints the record for `kiwi migrate import`.
    Provision(ProvisionArgs),
}

#[derive(Debug, clap::Args)]
pub struct ProvisionArgs {
    /// The builder's data directory (as for `run`)
    #[arg(long, env = "KIWI_BUILDER_DATA_DIR")]
    pub data_dir: Option<PathBuf>,
    /// The address the new Kiwi will have: https://kiwi.example.ts.net
    #[arg(long)]
    pub server: String,
    /// How Kiwi names this Mac [default: its computer name]
    #[arg(long)]
    pub name: Option<String>,
    /// Replace an existing pairing
    #[arg(long)]
    pub replace: bool,
}

#[derive(Debug, clap::Args)]
pub struct RunArgs {
    /// The builder's data directory [default: ~/Library/Application Support/Kiwi Builder on macOS, $XDG_DATA_HOME/kiwi-builder elsewhere]
    #[arg(long, env = "KIWI_BUILDER_DATA_DIR")]
    pub data_dir: Option<PathBuf>,
    /// Python 3.12 or newer for the runner
    #[arg(long, env = "KIWI_BUILDER_PYTHON", default_value = PYTHON)]
    pub python: PathBuf,
}

#[derive(Debug, clap::Args)]
pub struct PairArgs {
    /// The builder's data directory (as for `run`)
    #[arg(long, env = "KIWI_BUILDER_DATA_DIR")]
    pub data_dir: Option<PathBuf>,
    /// Kiwi's address: https://kiwi.example.ts.net (http:// only on this computer)
    #[arg(long)]
    pub server: String,
    /// The pairing code Kiwi shows (XXXX-XXXX-XXXX)
    #[arg(long)]
    pub code: String,
    /// How Kiwi names this Mac [default: its computer name]
    #[arg(long)]
    pub name: Option<String>,
}

/// `kiwi builder …`.
pub async fn main(command: Command) -> Result<(), String> {
    match command {
        Command::Pair(args) => {
            let data = args.data_dir.unwrap_or_else(default_data_dir);
            let server = server_url(&args.server)?;
            let name = args.name.unwrap_or_else(computer_name);
            let saved = pair(&http()?, &server, &args.code, &name, &HeaderMap::new()).await?;
            save(&data, &saved)?;
            println!(
                "Paired with Kiwi at {} as \"{}\".",
                saved.kiwi.as_deref().unwrap_or(saved.server.as_str()),
                saved.name
            );
            Ok(())
        }
        Command::Provision(args) => {
            let data = args.data_dir.unwrap_or_else(default_data_dir);
            let record = provision(&data, &args.server, args.name, args.replace)?;
            println!(
                "{}",
                serde_json::to_string_pretty(&record).map_err(|e| e.to_string())?
            );
            Ok(())
        }
        Command::Run(args) => {
            let data = args.data_dir.unwrap_or_else(default_data_dir);
            let _lock = lock(&data)?;
            let tools = home().join(TOOLS);
            prepare(&data, &tools)?;
            let stop = CancellationToken::new();
            tokio::spawn({
                let stop = stop.clone();
                async move {
                    shutdown_signal().await;
                    tracing::info!("stopping");
                    stop.cancel();
                }
            });
            let saved = loop {
                match load(&data) {
                    Ok(saved) => break saved,
                    Err(e) => {
                        tracing::error!("{e}");
                        tokio::select! {
                            _ = stop.cancelled() => return Ok(()),
                            _ = tokio::time::sleep(MAX_BACKOFF) => {}
                        }
                    }
                }
            };
            let builder = Builder::new(data, args.python, saved)?;
            tracing::info!(
                "Kiwi Builder {} for {} (data {})",
                env!("CARGO_PKG_VERSION"),
                builder.server,
                builder.data_dir.display()
            );
            builder.run(&stop).await;
            Ok(())
        }
    }
}

#[cfg(unix)]
async fn shutdown_signal() {
    let mut term = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .expect("SIGTERM handler");
    tokio::select! {
        _ = tokio::signal::ctrl_c() => {}
        _ = term.recv() => {}
    }
}
#[cfg(not(unix))]
async fn shutdown_signal() {
    let _ = tokio::signal::ctrl_c().await;
}

fn home() -> PathBuf {
    std::env::var_os("HOME").map_or_else(|| PathBuf::from("."), PathBuf::from)
}

/// `~/Library/Application Support/Kiwi Builder` on macOS, `$XDG_DATA_HOME/kiwi-builder`
/// (`~/.local/share/kiwi-builder`) elsewhere.
pub fn default_data_dir() -> PathBuf {
    if cfg!(target_os = "macos") {
        return home().join("Library/Application Support/Kiwi Builder");
    }
    std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .unwrap_or_else(|| home().join(".local/share"))
        .join("kiwi-builder")
}

/// Kiwi's address: `https://`, or `http://` to a loopback address (an SSH tunnel, tests).
/// An origin only, without a trailing slash.
pub fn server_url(text: &str) -> Result<reqwest::Url, String> {
    let url = reqwest::Url::parse(text.trim()).map_err(|e| format!("--server {text}: {e}"))?;
    let loopback = match url.host() {
        Some(url::Host::Domain(d)) => d.eq_ignore_ascii_case("localhost"),
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        None => false,
    };
    let scheme_ok = url.scheme() == "https" || (url.scheme() == "http" && loopback);
    if !scheme_ok
        || url.host().is_none()
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
        || url.path() != "/"
    {
        return Err(format!(
            "--server must be Kiwi's https:// address, such as https://kiwi.example.ts.net \
             (http:// only to this computer), got {text}"
        ));
    }
    Ok(url)
}

/// The HTTP client: certificates verified with the system's roots, no redirects, no proxy
/// (Kiwi is reached over the tailnet).
pub fn http() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .user_agent(concat!("Kiwi-Builder/", env!("CARGO_PKG_VERSION")))
        .connect_timeout(Duration::from_secs(15))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .build()
        .map_err(|e| e.to_string())
}

/// How Kiwi names this Mac by default: its computer name.
pub fn computer_name() -> String {
    let output = |program: &str, args: &[&str]| {
        std::process::Command::new(program)
            .args(args)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty())
    };
    let name = if cfg!(target_os = "macos") {
        output("/usr/sbin/scutil", &["--get", "ComputerName"])
    } else {
        output("hostname", &[])
    };
    let name: String = name
        .unwrap_or_else(|| "Mac".into())
        .chars()
        .filter(|c| !c.is_control())
        .take(64)
        .collect();
    name
}

/// "macOS 26.1 (arm64)".
pub fn system() -> String {
    let arch = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        other => other,
    };
    if cfg!(target_os = "macos") {
        let version = std::process::Command::new("/usr/bin/sw_vers")
            .arg("-productVersion")
            .output()
            .ok()
            .and_then(|o| String::from_utf8(o.stdout).ok())
            .map(|s| s.trim().to_owned())
            .filter(|s| !s.is_empty() && s.len() <= 32)
            .unwrap_or_else(|| "?".into());
        format!("macOS {version} ({arch})")
    } else {
        format!("{} ({arch})", std::env::consts::OS)
    }
}

/// `builder.json` in the data directory (0600): what pairing gave.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Saved {
    pub server: String,
    pub token: String,
    pub name: String,
    #[serde(default)]
    pub kiwi: Option<String>,
}

pub fn load(data: &Path) -> Result<Saved, String> {
    let path = data.join(SAVED);
    let text = std::fs::read_to_string(&path).map_err(|e| {
        format!(
            "{}: {e}. Pair this Mac first: ./install.sh --builder --server <Kiwi's address> --pair <code>",
            path.display()
        )
    })?;
    serde_json::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))
}

pub fn save(data: &Path, saved: &Saved) -> Result<(), String> {
    let error = |e: std::io::Error| format!("{}: {e}", data.display());
    create_private_dir(data).map_err(error)?;
    let temporary = data.join(format!("{SAVED}.new"));
    let _ = std::fs::remove_file(&temporary);
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(&temporary).map_err(error)?;
    std::io::Write::write_all(
        &mut file,
        &serde_json::to_vec_pretty(saved).map_err(|e| e.to_string())?,
    )
    .and_then(|()| file.sync_all())
    .map_err(error)?;
    std::fs::rename(&temporary, data.join(SAVED)).map_err(error)
}

fn create_private_dir(path: &Path) -> std::io::Result<()> {
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(path)
}

/// Takes `<data>/builder.lock` while the builder runs, so two builders never share a data
/// directory. (It takes no lock of a Kiwi's data directory: both may run on one Mac.)
pub fn lock(data: &Path) -> Result<std::fs::File, String> {
    let path = data.join("builder.lock");
    let error = |e: std::io::Error| format!("{}: {e}", path.display());
    create_private_dir(data).map_err(error)?;
    let file = std::fs::File::options()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(&path)
        .map_err(error)?;
    match file.try_lock() {
        Ok(()) => Ok(file),
        Err(std::fs::TryLockError::WouldBlock) => Err(format!(
            "Another Kiwi Builder is already running with data directory {}",
            data.display()
        )),
        Err(std::fs::TryLockError::Error(e)) => Err(error(e)),
    }
}

/// Writes the embedded runner and recipes into `<data>/automation` (as Kiwi does) and
/// links `automation/tools` to `tools` unless it exists.
pub fn prepare(data: &Path, tools: &Path) -> Result<(), String> {
    let root = data.join("automation");
    let error = |e: std::io::Error| format!("{}: {e}", root.display());
    create_private_dir(&root).map_err(error)?;
    for (name, text) in [
        ("runner.py", automation::IOS_RUNNER),
        ("recipes.json", automation::IOS_RECIPES),
    ] {
        let path = root.join(name);
        if std::fs::read_to_string(&path).ok().as_deref() != Some(text) {
            let temporary = root.join(format!("{name}.new"));
            std::fs::write(&temporary, text).map_err(error)?;
            std::fs::rename(temporary, path).map_err(error)?;
        }
    }
    let link = root.join("tools");
    #[cfg(unix)]
    if link.symlink_metadata().is_err() {
        create_private_dir(tools).map_err(error)?;
        std::os::unix::fs::symlink(tools, &link).map_err(error)?;
    }
    #[cfg(not(unix))]
    let _ = (link, tools);
    Ok(())
}

/// What went wrong with a call to Kiwi.
#[derive(Debug)]
enum CallError {
    /// `401`: the token is unknown or revoked.
    Unpaired(String),
    /// `410`: Kiwi no longer has the task.
    Gone,
    /// Another answer from Kiwi: its status and message.
    Refused(StatusCode, String),
    /// No answer.
    Network(String),
}

impl std::fmt::Display for CallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unpaired(m) => write!(f, "not paired: {m}"),
            Self::Gone => f.write_str("Kiwi no longer has the task"),
            Self::Refused(status, m) => write!(f, "Kiwi answered {}: {m}", status.as_u16()),
            Self::Network(m) => write!(f, "could not reach Kiwi: {m}"),
        }
    }
}

/// Kiwi's answer: its status and JSON body (`null` when empty).
async fn answer(response: reqwest::Response) -> Result<(StatusCode, Value), CallError> {
    let status = response.status();
    let bytes = response
        .bytes()
        .await
        .map_err(|e| CallError::Network(e.to_string()))?;
    if bytes.len() > 2 * TASK_LIMIT {
        return Err(CallError::Refused(status, "answer too large".into()));
    }
    let body = if bytes.is_empty() {
        Value::Null
    } else {
        serde_json::from_slice(&bytes).unwrap_or(Value::Null)
    };
    let message = || {
        body["error"]["message"]
            .as_str()
            .unwrap_or("no message")
            .to_owned()
    };
    match status {
        s if s.is_success() => Ok((status, body)),
        StatusCode::UNAUTHORIZED => Err(CallError::Unpaired(message())),
        StatusCode::GONE => Err(CallError::Gone),
        _ => Err(CallError::Refused(status, message())),
    }
}

fn network(e: reqwest::Error) -> CallError {
    CallError::Network(e.to_string())
}

/// `POST /api/builder/pair`.
pub async fn pair(
    http: &reqwest::Client,
    server: &reqwest::Url,
    code: &str,
    name: &str,
    headers: &HeaderMap,
) -> Result<Saved, String> {
    let hello = Hello::own(system());
    let mut body = serde_json::to_value(&hello).map_err(|e| e.to_string())?;
    body["code"] = json!(code);
    body["name"] = json!(name);
    let response = http
        .post(server.join("api/builder/pair").map_err(|e| e.to_string())?)
        .headers(headers.clone())
        .header("X-Kiwi", "1")
        .json(&body)
        .timeout(Duration::from_secs(60))
        .send()
        .await
        .map_err(|e| format!("Could not reach Kiwi at {server}: {e}"))?;
    let (_, body) = answer(response).await.map_err(|e| match e {
        CallError::Refused(_, message) | CallError::Unpaired(message) => {
            format!("Kiwi refused to pair: {message}")
        }
        other => other.to_string(),
    })?;
    let token = body["token"]
        .as_str()
        .filter(|t| t.len() >= 43)
        .ok_or("Kiwi gave no token")?;
    Ok(Saved {
        server: server.as_str().trim_end_matches('/').to_owned(),
        token: token.to_owned(),
        name: body["name"].as_str().unwrap_or(name).to_owned(),
        kiwi: body["kiwi"].as_str().map(str::to_owned),
    })
}

/// `kiwi builder provision`: saves a pairing with a new token for `server` in `data` and
/// returns the record the Kiwi there imports.
pub fn provision(
    data: &Path,
    server: &str,
    name: Option<String>,
    replace: bool,
) -> Result<Value, String> {
    let server = server_url(server)?;
    if !replace && data.join(SAVED).exists() {
        return Err(format!(
            "{} is already paired with a Kiwi; pass --replace to pair it with {server} instead",
            data.display()
        ));
    }
    let name = name.unwrap_or_else(computer_name);
    let (token, record) = super::provision(&name, Hello::own(system()))?;
    let server = server.as_str().trim_end_matches('/').to_owned();
    save(
        data,
        &Saved {
            server: server.clone(),
            token,
            name: name.trim().to_owned(),
            kiwi: Some(server),
        },
    )?;
    Ok(record)
}

/// A task Kiwi sent, after checking it.
#[derive(Debug)]
struct Task {
    id: String,
    command: String,
    recipe: Option<String>,
    request: Value,
    timeout: Duration,
}

/// Checks a task: a known command, an iOS recipe of this builder's own, an id that names a
/// folder safely, a JSON object as request.
fn task(value: &Value) -> Result<Task, String> {
    let command = value["command"].as_str().unwrap_or("");
    if !matches!(command, "plan" | "build" | "doctor") {
        return Err(format!("Unknown task command {command:?}"));
    }
    let recipe = match value["recipe"].as_str() {
        Some(id) => {
            if !automation::recipe(id).is_ok_and(|r| !r.android()) {
                return Err(format!("Kiwi Builder doesn't build {id:?}"));
            }
            Some(id.to_owned())
        }
        None if command == "doctor" => None,
        None => return Err("The task names no recipe".into()),
    };
    let request = value["request"].clone();
    if !request.is_object() || request.to_string().len() > TASK_LIMIT {
        return Err("Invalid task request".into());
    }
    let timeout = value["deadline"]
        .as_str()
        .and_then(|d| chrono::DateTime::parse_from_rfc3339(d).ok())
        .and_then(|d| (d.with_timezone(&Utc) - Utc::now()).to_std().ok())
        .unwrap_or(Duration::from_secs(1))
        .clamp(Duration::from_secs(1), Duration::from_secs(7300));
    Ok(Task {
        id: task_id(value).ok_or("Invalid task id")?,
        command: command.into(),
        recipe,
        request,
        timeout,
    })
}

fn task_id(value: &Value) -> Option<String> {
    value["id"]
        .as_str()
        .filter(|id| {
            id.len() == 24
                && id
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
        .map(str::to_owned)
}

/// How forwarding a task's events ended.
#[derive(Debug, PartialEq, Eq)]
enum Forwarded {
    /// Every event reached Kiwi.
    Delivered,
    /// Kiwi asked to stop the runner.
    Cancelled,
    /// Kiwi no longer has the task, or the builder is no longer paired.
    Gone,
    /// Kiwi couldn't be reached for too long.
    Unreachable,
}

/// The builder's connection to Kiwi and how it runs tasks.
pub struct Builder {
    pub server: reqwest::Url,
    token: RwLock<String>,
    pub name: String,
    pub data_dir: PathBuf,
    pub python: PathBuf,
    /// The runner it runs: `<data>/automation/runner.py` (tests change it).
    pub runner: PathBuf,
    /// What it reports (tests change it).
    pub hello: Hello,
    /// How often it tells Kiwi a quiet task is alive.
    pub heartbeat: Duration,
    /// Sent with every call (tests: a forwarded tailnet address).
    pub headers: HeaderMap,
    http: reqwest::Client,
}

impl Builder {
    pub fn new(data_dir: PathBuf, python: PathBuf, saved: Saved) -> Result<Self, String> {
        let server = server_url(&saved.server)?;
        Ok(Self {
            server,
            token: RwLock::new(saved.token),
            name: saved.name,
            runner: data_dir.join("automation/runner.py"),
            data_dir,
            python,
            hello: Hello::own(system()),
            heartbeat: Duration::from_secs(10),
            headers: HeaderMap::new(),
            http: http()?,
        })
    }

    fn request(&self, method: Method, path: &str) -> reqwest::RequestBuilder {
        let url = self
            .server
            .join(path)
            .unwrap_or_else(|_| self.server.clone());
        self.http
            .request(method, url)
            .headers(self.headers.clone())
            .header("X-Kiwi", "1")
            .bearer_auth(self.token.read().unwrap().as_str())
    }

    async fn post(
        &self,
        path: &str,
        body: &Value,
        timeout: Duration,
    ) -> Result<(StatusCode, Value), CallError> {
        let response = self
            .request(Method::POST, path)
            .json(body)
            .timeout(timeout)
            .send()
            .await
            .map_err(network)?;
        answer(response).await
    }

    /// Asks Kiwi for work until `stop` is cancelled, backing off (1 s to 60 s) while it
    /// can't be reached or refuses.
    pub async fn run(&self, stop: &CancellationToken) {
        let mut backoff = Duration::from_secs(1);
        tidy(&self.data_dir).await;
        while !stop.is_cancelled() {
            let mut body = serde_json::to_value(&self.hello).unwrap_or_default();
            body["name"] = json!(self.name);
            let polled = tokio::select! {
                _ = stop.cancelled() => break,
                polled = self.post("api/builder/poll", &body, Duration::from_secs(90)) => polled,
            };
            let wait = match polled {
                Ok((StatusCode::OK, answer)) => {
                    backoff = Duration::from_secs(1);
                    self.execute(&answer["task"], stop).await;
                    continue;
                }
                Ok(_) => {
                    backoff = Duration::from_secs(1);
                    continue;
                }
                Err(CallError::Unpaired(message)) => {
                    tracing::error!(
                        "Kiwi doesn't know this Mac builder ({message}). Pair it again: \
                         ./install.sh --builder --server {} --pair <code>",
                        self.server
                    );
                    // A new pairing (install.sh restarts the builder too) takes effect.
                    if let Ok(saved) = load(&self.data_dir)
                        && saved.token != *self.token.read().unwrap()
                    {
                        *self.token.write().unwrap() = saved.token;
                        continue;
                    }
                    MAX_BACKOFF
                }
                Err(e) => {
                    tracing::warn!("{e}; trying again in {}s", backoff.as_secs());
                    let wait = backoff;
                    backoff = (backoff * 2).min(MAX_BACKOFF);
                    wait
                }
            };
            tokio::select! {
                _ = stop.cancelled() => break,
                _ = tokio::time::sleep(wait) => {}
            }
        }
    }

    /// Runs one task: the runner with the task's request, its events forwarded as they
    /// arrive; for a build the IPA uploaded; then how it ended. Local artifacts are deleted
    /// afterwards, whatever happened.
    async fn execute(&self, value: &Value, stop: &CancellationToken) {
        let Some(id) = task_id(value) else {
            tracing::error!("Kiwi sent a task without a valid id; ignoring it");
            return;
        };
        let task = match task(value) {
            Ok(task) => task,
            Err(e) => {
                tracing::error!("refused task {id}: {e}");
                let message = format!("{} refused the task: {e}", self.name);
                self.finish(&id, "failed", Some(&message), None).await;
                return;
            }
        };
        tracing::info!(
            "task {}: {} {}",
            task.id,
            task.command,
            task.recipe.as_deref().unwrap_or("")
        );
        let root = self.data_dir.join("automation");
        let request = root.join(format!("{}.request.json", task.id));
        let mut artifacts = Vec::new();
        let outcome = self
            .execute_task(&task, &request, stop, &mut artifacts)
            .await;
        let _ = tokio::fs::remove_file(&request).await;
        for sha in artifacts {
            for extension in ["ipa", "json"] {
                let _ =
                    tokio::fs::remove_file(root.join(format!("artifacts/{sha}.{extension}"))).await;
            }
        }
        tracing::info!("task {}: {outcome}", task.id);
        tidy(&self.data_dir).await;
    }

    async fn execute_task(
        &self,
        task: &Task,
        request: &Path,
        stop: &CancellationToken,
        artifacts: &mut Vec<String>,
    ) -> &'static str {
        let written = match request.parent() {
            Some(root) => tokio::fs::create_dir_all(root).await,
            None => Ok(()),
        };
        if let Err(e) = match written {
            Ok(()) => write_private(request, task.request.to_string().as_bytes()).await,
            Err(e) => Err(e),
        } {
            let message = format!("{} could not write the task's request: {e}", self.name);
            self.finish(&task.id, "failed", Some(&message), None).await;
            return "failed";
        }
        let token = stop.child_token();
        let (sender, receiver) = mpsc::unbounded_channel();
        let run = process::Run {
            python: &self.python,
            program: &self.runner,
            command: &task.command,
            data_dir: &self.data_dir,
            request,
            id: &task.id,
            recipe: task.recipe.as_deref(),
            builder: "The Mac builder",
            timeout: task.timeout,
        };
        let running = async {
            let sender = sender;
            process::run(&run, &token, |event| {
                if event["event"] == "artifact"
                    && let Some(sha) = event["sha256"].as_str().filter(|s| valid_sha(s))
                {
                    artifacts.push(sha.to_owned());
                }
                let _ = sender.send(event);
                Ok(())
            })
            .await
        };
        let (result, forwarded) = tokio::join!(running, self.forward(&task.id, receiver, &token));
        match forwarded {
            Forwarded::Gone | Forwarded::Unreachable => return "dropped",
            Forwarded::Cancelled => {
                self.finish(&task.id, "cancelled", None, None).await;
                return "cancelled by Kiwi";
            }
            Forwarded::Delivered => {}
        }
        if stop.is_cancelled() {
            let message = format!("{} stopped.", self.name);
            self.finish(&task.id, "cancelled", Some(&message), None)
                .await;
            return "cancelled";
        }
        match result {
            Err((message, _)) => {
                self.finish(&task.id, "failed", Some(&message), None).await;
                "failed"
            }
            Ok(false) => {
                self.finish(&task.id, "failed", None, None).await;
                "failed"
            }
            Ok(true) if task.command == "build" => {
                let Some(sha) = artifacts.last().cloned() else {
                    self.finish(
                        &task.id,
                        "failed",
                        Some("The builder returned no patched app"),
                        None,
                    )
                    .await;
                    return "failed";
                };
                match self.upload(&task.id, &sha).await {
                    Ok(()) => {}
                    Err(CallError::Gone | CallError::Unpaired(_)) => return "dropped",
                    Err(e) => {
                        let message =
                            format!("{} could not upload the patched app: {e}", self.name);
                        self.finish(&task.id, "failed", Some(&message), None).await;
                        return "failed";
                    }
                }
                let provenance = tokio::fs::read(
                    self.data_dir
                        .join("automation/artifacts")
                        .join(format!("{sha}.json")),
                )
                .await
                .ok()
                .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
                .filter(|p| p["sha256"].as_str() == Some(sha.as_str()));
                self.finish(&task.id, "ok", None, provenance).await;
                "built"
            }
            Ok(true) => {
                self.finish(&task.id, "ok", None, None).await;
                "done"
            }
        }
    }

    /// Sends the runner's events to Kiwi as they arrive, an empty list as heartbeat while
    /// it is quiet. Cancels `token` (stopping the runner) when Kiwi asks to, no longer has
    /// the task, or can't be reached for too long.
    async fn forward(
        &self,
        id: &str,
        mut receiver: mpsc::UnboundedReceiver<Value>,
        token: &CancellationToken,
    ) -> Forwarded {
        let path = format!("api/builder/tasks/{id}/events");
        let mut outcome = Forwarded::Delivered;
        let mut closed = false;
        while !closed {
            let mut batch = Vec::new();
            tokio::select! {
                event = receiver.recv() => match event {
                    Some(event) => batch.push(event),
                    None => closed = true,
                },
                _ = tokio::time::sleep(self.heartbeat) => {}
            }
            while batch.len() < 256 {
                match receiver.try_recv() {
                    Ok(event) => batch.push(event),
                    Err(mpsc::error::TryRecvError::Empty) => break,
                    Err(mpsc::error::TryRecvError::Disconnected) => {
                        closed = true;
                        break;
                    }
                }
            }
            if outcome != Forwarded::Delivered || (closed && batch.is_empty()) {
                continue;
            }
            let body = json!({ "events": batch });
            let started = tokio::time::Instant::now();
            let mut backoff = Duration::from_secs(1);
            loop {
                match self.post(&path, &body, Duration::from_secs(60)).await {
                    Ok((_, answer)) => {
                        if answer["cancel"] == true {
                            tracing::info!("Kiwi cancelled task {id}");
                            outcome = Forwarded::Cancelled;
                            token.cancel();
                        }
                        break;
                    }
                    Err(CallError::Gone | CallError::Unpaired(_)) => {
                        tracing::info!("Kiwi no longer has task {id}: stopping it");
                        outcome = Forwarded::Gone;
                        token.cancel();
                        break;
                    }
                    Err(e) if started.elapsed() < GIVE_UP => {
                        tracing::warn!("task {id}: {e}; trying again");
                        tokio::time::sleep(backoff).await;
                        backoff = (backoff * 2).min(Duration::from_secs(15));
                    }
                    Err(e) => {
                        tracing::error!("task {id}: {e}; giving it up");
                        outcome = Forwarded::Unreachable;
                        token.cancel();
                        break;
                    }
                }
            }
        }
        outcome
    }

    /// `PUT …/artifact`: streams `artifacts/<sha>.ipa`, trying again a few times when the
    /// connection fails.
    async fn upload(&self, id: &str, sha: &str) -> Result<(), CallError> {
        let path = self
            .data_dir
            .join("automation/artifacts")
            .join(format!("{sha}.ipa"));
        let mut attempt = 0;
        loop {
            attempt += 1;
            let file = tokio::fs::File::open(&path).await.map_err(|e| {
                CallError::Refused(StatusCode::NOT_FOUND, format!("{}: {e}", path.display()))
            })?;
            let length = file
                .metadata()
                .await
                .map_err(|e| CallError::Refused(StatusCode::NOT_FOUND, e.to_string()))?
                .len();
            let body = reqwest::Body::wrap_stream(tokio_util::io::ReaderStream::new(file));
            let sent = self
                .request(
                    Method::PUT,
                    &format!("api/builder/tasks/{id}/artifact?sha256={sha}"),
                )
                .header(CONTENT_TYPE, "application/octet-stream")
                .header(CONTENT_LENGTH, length)
                .body(body)
                .send()
                .await;
            let error = match sent {
                Ok(response) => return answer(response).await.map(drop),
                Err(e) => e,
            };
            // Kiwi may have refused it before reading it all (the task is gone or was
            // cancelled), which breaks the connection: ask before sending it again.
            let events = format!("api/builder/tasks/{id}/events");
            match self
                .post(&events, &json!({ "events": [] }), Duration::from_secs(60))
                .await
            {
                Ok((_, answer)) if answer["cancel"] == true => return Err(CallError::Gone),
                Err(e @ (CallError::Gone | CallError::Unpaired(_))) => return Err(e),
                _ => {}
            }
            if attempt >= 3 {
                return Err(network(error));
            }
            tracing::warn!("uploading the patched app of task {id} failed: {error}; trying again");
            tokio::time::sleep(Duration::from_secs(5 * attempt)).await;
        }
    }

    /// `POST …/finish`, trying again while Kiwi can't be reached.
    async fn finish(&self, id: &str, status: &str, error: Option<&str>, provenance: Option<Value>) {
        let body = json!({ "status": status, "error": error, "provenance": provenance });
        let path = format!("api/builder/tasks/{id}/finish");
        let started = tokio::time::Instant::now();
        let mut backoff = Duration::from_secs(1);
        loop {
            match self.post(&path, &body, Duration::from_secs(60)).await {
                Ok(_) | Err(CallError::Gone | CallError::Unpaired(_)) => return,
                Err(CallError::Refused(status, message)) => {
                    tracing::error!(
                        "Kiwi refused how task {id} ended ({}): {message}",
                        status.as_u16()
                    );
                    return;
                }
                Err(e) if started.elapsed() < GIVE_UP => {
                    tracing::warn!("task {id}: {e}; trying again");
                    tokio::time::sleep(backoff).await;
                    backoff = (backoff * 2).min(Duration::from_secs(15));
                }
                Err(e) => {
                    tracing::error!("task {id}: {e}; giving up");
                    return;
                }
            }
        }
    }
}

/// Removes what tasks leave behind on the Mac: build logs (`automation/jobs/<id>`) after 30
/// days, artifacts a crash kept (`automation/artifacts`) after a day. The toolchain's job
/// folders are shared with a Kiwi on the same Mac and left to the runner.
async fn tidy(data: &Path) {
    let root = data.join("automation");
    let _ = tokio::task::spawn_blocking(move || {
        let old = |path: &Path, days: u64| {
            path.symlink_metadata()
                .ok()
                .filter(|m| !m.file_type().is_symlink())
                .and_then(|m| m.modified().ok())
                .and_then(|t| t.elapsed().ok())
                .is_some_and(|age| age > Duration::from_secs(days * 86400))
        };
        for entry in std::fs::read_dir(root.join("jobs"))
            .into_iter()
            .flatten()
            .flatten()
        {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            if name.len() == 24
                && name.bytes().all(|b| b.is_ascii_hexdigit())
                && old(&entry.path(), 30)
            {
                let _ = std::fs::remove_dir_all(entry.path());
            }
        }
        for entry in std::fs::read_dir(root.join("artifacts"))
            .into_iter()
            .flatten()
            .flatten()
        {
            let path = entry.path();
            let stem = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            if valid_sha(stem) && old(&path, 1) {
                let _ = std::fs::remove_file(path);
            }
        }
    })
    .await;
}

async fn write_private(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut options = tokio::fs::OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    options.mode(0o600);
    let mut file = options.open(path).await?;
    tokio::io::AsyncWriteExt::write_all(&mut file, bytes).await?;
    file.sync_all().await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_https_or_loopback_http_servers() {
        assert!(server_url("https://kiwi.example.ts.net").is_ok());
        assert!(server_url("https://kiwi.example.ts.net/").is_ok());
        assert!(server_url("http://127.0.0.1:18742").is_ok());
        assert!(server_url("http://localhost:18742").is_ok());
        assert!(server_url("http://[::1]:18742").is_ok());
        for bad in [
            "http://kiwi.example.ts.net",
            "http://100.64.0.1:8742",
            "https://user:pw@kiwi.example.ts.net",
            "https://kiwi.example.ts.net/api",
            "https://kiwi.example.ts.net/?x=1",
            "ftp://kiwi.example.ts.net",
            "kiwi.example.ts.net",
        ] {
            assert!(server_url(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn tasks_are_checked_before_they_run() {
        let deadline = (Utc::now() + chrono::Duration::minutes(10)).to_rfc3339();
        let ok = json!({"id": "a".repeat(24), "command": "plan", "recipe": "youtube",
            "request": {"app": "youtube"}, "deadline": deadline});
        let t = task(&ok).unwrap();
        assert_eq!(t.recipe.as_deref(), Some("youtube"));
        assert!(t.timeout > Duration::from_secs(500) && t.timeout <= Duration::from_secs(600));
        let doctor = json!({"id": "b".repeat(24), "command": "doctor", "recipe": null,
            "request": {}, "deadline": deadline});
        assert!(task(&doctor).unwrap().recipe.is_none());
        for (field, value) in [
            ("command", json!("setup")),
            ("command", json!("enroll")),
            ("recipe", json!("android-youtube")),
            ("recipe", json!("../youtube")),
            ("recipe", Value::Null),
            ("request", json!([1])),
            ("id", json!("../../etc")),
            ("id", json!("A".repeat(24))),
        ] {
            let mut bad = ok.clone();
            bad[field] = value;
            assert!(task(&bad).is_err(), "{field}: {bad}");
        }
    }

    #[test]
    fn prepare_writes_the_runner_and_links_the_tools() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("Kiwi Builder");
        let tools = dir.path().join("tools");
        prepare(&data, &tools).unwrap();
        assert_eq!(
            std::fs::read_to_string(data.join("automation/runner.py")).unwrap(),
            automation::IOS_RUNNER
        );
        #[cfg(unix)]
        assert_eq!(
            std::fs::read_link(data.join("automation/tools")).unwrap(),
            tools
        );
        // A second start keeps them.
        prepare(&data, &tools).unwrap();
        let saved = Saved {
            server: "https://kiwi.example.ts.net".into(),
            token: "t".repeat(43),
            name: "Mac".into(),
            kiwi: None,
        };
        save(&data, &saved).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(data.join(SAVED))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert_eq!(load(&data).unwrap().token, saved.token);
        let _held = lock(&data).unwrap();
        assert!(lock(&data).is_err());
    }
}
