//! The Mac builder: a Mac that builds iOS apps for a Kiwi that doesn't run on macOS
//! (docs/kiwi-builds.md "Mac builder", api.md "Mac builder").
//!
//! The Mac runs `kiwi builder run` ([`client`]), which connects out to Kiwi, asks for work
//! with a long poll and runs only its own installed runner.py. Kiwi hands it iOS
//! `plan`/`build`/`doctor` calls as tasks instead of starting a local runner
//! ([`run_task`]), applies the events the Mac forwards exactly as a local runner's
//! ([`crate::automation::process::Events`]), takes the patched IPA (checked against its
//! SHA-256) and then validates, signs and publishes it as always. One builder per Kiwi;
//! pairing another replaces it.
//!
//! The pairing code and the tasks live in memory: a restart ends the tasks (the builder's
//! next call gets `410` and it stops its runner). The paired builder (its name, the
//! SHA-256 of its token, what it reported) is saved in `meta` (`mac_builder`).
use std::{
    collections::{HashSet, VecDeque},
    sync::{
        Arc, Mutex, RwLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

use axum::{
    body::Body,
    extract::{ConnectInfo, FromRequestParts},
    http::{StatusCode, header, request::Parts},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use chrono::Utc;
use futures_util::StreamExt;
use kiwi_source::CancellationToken;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use tokio::{io::AsyncWriteExt, sync::Notify, sync::oneshot};

use crate::{
    Kiwi,
    automation::{self, Failure, Job, failed, process::Events, valid_sha},
    db::{Db, Time},
    error::{ApiError, ApiResult},
    peers::{self, Connection},
};

pub mod client;

/// The setup check that tells whether the paired Mac builder can build now.
pub const CHECK: &str = "Mac builder";
const META: &str = "mac_builder";
/// Pairing codes: 12 characters of these 32 (60 bits), shown as `XXXX-XXXX-XXXX`.
const CODE_ALPHABET: &[u8; 32] = b"23456789ABCDEFGHJKLMNPQRSTUVWXYZ";
const CODE_LENGTH: usize = 12;
/// Wrong pairing codes allowed per [`PAIR_WINDOW`] before pairing answers `429`.
const PAIR_FAILURES: usize = 5;
const PAIR_WINDOW: Duration = Duration::from_secs(600);
/// The largest IPA a builder may upload.
pub const ARTIFACT_LIMIT: u64 = 4 * 1024 * 1024 * 1024;
/// The most `last_seen` lags behind in the database (it is exact in memory).
const SEEN_SAVE_EVERY: Duration = Duration::from_secs(300);
pub const UNSUPPORTED: &str = "This Kiwi runs on a Mac and builds iOS apps itself.";

/// How long things take; tests shorten them.
#[derive(Clone, Debug)]
pub struct Timing {
    /// A builder heard from within this long is online.
    pub online: Duration,
    /// A task without a call from the builder for this long is lost.
    pub lost: Duration,
    /// How long a poll waits for a task.
    pub poll: Duration,
    /// How often the builder runs `doctor` (also when it connects and when asked).
    pub doctor_every: Duration,
    pub doctor_timeout: Duration,
    /// How long a pairing code works.
    pub code_lifetime: chrono::Duration,
}

impl Default for Timing {
    fn default() -> Self {
        Self {
            online: Duration::from_secs(60),
            lost: Duration::from_secs(120),
            poll: Duration::from_secs(25),
            doctor_every: Duration::from_secs(6 * 3600),
            doctor_timeout: Duration::from_secs(300),
            code_lifetime: chrono::Duration::minutes(10),
        }
    }
}

/// What a builder reports about itself when it pairs and polls.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hello {
    /// The kiwi crate version it was built from.
    pub version: String,
    /// "macOS 26.1 (arm64)".
    pub os: String,
    /// SHA-256 of the runner.py and recipes.json it carries.
    pub runner_sha256: String,
    pub recipes_sha256: String,
}

impl Hello {
    /// This program's own.
    pub fn own(os: String) -> Self {
        Self {
            version: env!("CARGO_PKG_VERSION").into(),
            os,
            runner_sha256: automation::RUNNER_SHA256.clone(),
            recipes_sha256: automation::RECIPES_SHA256.clone(),
        }
    }

    fn validate(&self) -> ApiResult<()> {
        if !text_ok(&self.version, 32) || !text_ok(&self.os, 100) {
            return Err(ApiError::bad_request("Invalid builder version or system."));
        }
        if !valid_sha(&self.runner_sha256) || !valid_sha(&self.recipes_sha256) {
            return Err(ApiError::bad_request("Invalid runner or recipes digest."));
        }
        Ok(())
    }

    /// Whether it runs the runner and recipes this Kiwi carries.
    fn compatible(&self) -> bool {
        self.runner_sha256 == *automation::RUNNER_SHA256
            && self.recipes_sha256 == *automation::RECIPES_SHA256
    }
}

/// Printable text of 1 to `max` characters.
fn text_ok(text: &str, max: usize) -> bool {
    !text.trim().is_empty()
        && text.chars().count() <= max
        && !text.chars().any(|c| c.is_control() || bidi_control(c))
}

/// Characters that reorder text (a name that reads differently than it is).
fn bidi_control(c: char) -> bool {
    matches!(c, '\u{200e}' | '\u{200f}' | '\u{202a}'..='\u{202e}' | '\u{2066}'..='\u{2069}')
}

/// The paired builder, as saved.
#[derive(Clone, Debug, Serialize, Deserialize)]
struct Paired {
    name: String,
    token_sha256: String,
    paired_at: Time,
    #[serde(default)]
    last_seen: Option<Time>,
    #[serde(flatten)]
    hello: Hello,
}

/// One call of the runner on the Mac.
struct Task {
    id: String,
    command: String,
    recipe: Option<String>,
    request: Value,
    deadline: Time,
    /// The Kiwi Builds job it belongs to (plan and build); `None` for doctor.
    job: Option<String>,
    created_at: Time,
    /// When the builder took it.
    started_at: Option<Time>,
    /// The last call the builder made for it (or when it was made, until then).
    contact: Instant,
    /// Kiwi doesn't want it any more; the builder hears so at its next call.
    cancelled: bool,
    /// `None` while a call applies events.
    events: Option<Events>,
    bytes: usize,
    /// The SHA-256 of the `artifact` event, and of the upload that matched it.
    artifact: Option<String>,
    uploaded: Option<String>,
    uploading: bool,
    /// The waiting [`run_task`].
    done: Option<oneshot::Sender<Result<Vec<Value>, Failure>>>,
}

impl Task {
    fn new(
        id: String,
        command: &str,
        recipe: Option<String>,
        request: Value,
        timeout: Duration,
        job: Option<String>,
        done: Option<oneshot::Sender<Result<Vec<Value>, Failure>>>,
    ) -> Self {
        let now = Utc::now();
        Self {
            id,
            command: command.into(),
            recipe,
            request,
            deadline: now + chrono::Duration::from_std(timeout).unwrap_or(chrono::Duration::MAX),
            job,
            created_at: now,
            started_at: None,
            contact: Instant::now(),
            cancelled: false,
            events: Some(Events::default()),
            bytes: 0,
            artifact: None,
            uploaded: None,
            uploading: false,
            done,
        }
    }

    fn complete(&mut self, result: Result<Vec<Value>, Failure>) {
        if let Some(done) = self.done.take() {
            let _ = done.send(result);
        }
    }

    fn json(&self) -> Value {
        json!({"id": self.id, "command": self.command, "recipe": self.recipe,
            "request": self.request, "deadline": self.deadline})
    }
}

#[derive(Default)]
struct Inner {
    paired: Option<Paired>,
    /// Counts pairings, so a replaced builder's poll that was waiting takes no task.
    generation: u64,
    /// The unused pairing code (normalized) and when it expires.
    pairing: Option<(String, Time)>,
    failures: VecDeque<Instant>,
    /// The builder's last call.
    contact: Option<Instant>,
    seen_saved: Option<Instant>,
    tasks: Vec<Task>,
    doctor_requested: bool,
    /// When the last doctor task was handed out (since the builder connected).
    doctor_at: Option<Instant>,
}

impl Inner {
    fn online(&self, timing: &Timing) -> bool {
        self.paired.is_some() && self.contact.is_some_and(|t| t.elapsed() < timing.online)
    }

    fn task(&mut self, id: &str) -> Option<&mut Task> {
        self.tasks.iter_mut().find(|t| t.id == id)
    }

    fn name(&self) -> String {
        self.paired
            .as_ref()
            .map_or_else(|| "the Mac builder".into(), |p| p.name.clone())
    }

    /// Ends every task with `message` (the builder was removed or replaced).
    fn end_tasks(&mut self, message: &str, retryable: &mut HashSet<String>) {
        for mut task in self.tasks.drain(..) {
            if let Some(job) = &task.job
                && task.done.is_some()
            {
                retryable.insert(job.clone());
            }
            task.complete(Err(failed(message)));
        }
    }
}

/// The Mac builder's state in a Kiwi.
pub struct MacBuilder {
    /// Whether this Kiwi pairs a Mac builder: everywhere but on macOS, which builds iOS apps
    /// itself. Tests turn it on.
    pub supported: AtomicBool,
    pub timing: RwLock<Timing>,
    inner: Mutex<Inner>,
    /// Jobs whose task was lost or ended by the builder: retryable failures that don't
    /// count against the recipe.
    retryable: Mutex<HashSet<String>>,
    /// Wakes polls when there is a task.
    wake: Notify,
}

impl MacBuilder {
    pub fn load(db: &Db) -> Result<Self, String> {
        let paired = match db.meta(META).map_err(|e| e.to_string())? {
            Some(text) => serde_json::from_str::<Option<Paired>>(&text)
                .map_err(|e| format!("Mac builder: {e}"))?,
            None => None,
        };
        Ok(Self {
            supported: AtomicBool::new(!cfg!(target_os = "macos")),
            timing: RwLock::default(),
            inner: Mutex::new(Inner {
                paired,
                ..Inner::default()
            }),
            retryable: Mutex::default(),
            wake: Notify::new(),
        })
    }

    pub fn supported(&self) -> bool {
        self.supported.load(Ordering::Relaxed)
    }

    fn timing(&self) -> Timing {
        self.timing.read().unwrap().clone()
    }

    /// Whether a builder is paired (on a Kiwi that pairs one).
    pub fn paired(&self) -> bool {
        self.supported() && self.inner.lock().unwrap().paired.is_some()
    }

    /// Online and running the same runner and recipes: iOS jobs can start.
    pub fn ready(&self) -> bool {
        self.wait_reason().is_none()
    }

    /// Why iOS jobs wait (their stage), `None` when the builder can take them.
    pub fn wait_reason(&self) -> Option<String> {
        let timing = self.timing();
        let inner = self.inner.lock().unwrap();
        let paired = match &inner.paired {
            Some(p) if self.supported() => p,
            _ => return Some("Waiting for a Mac builder".into()),
        };
        if !inner.online(&timing) {
            Some(format!("Waiting for {}", paired.name))
        } else if !paired.hello.compatible() {
            Some(format!("Waiting for an update of Kiwi on {}", paired.name))
        } else {
            None
        }
    }

    /// Whether the job's task was lost (and forgets it).
    pub fn take_retryable(&self, job: &str) -> bool {
        self.retryable.lock().unwrap().remove(job)
    }

    fn save(&self, db: &Db, paired: Option<&Paired>) -> ApiResult<()> {
        db.set_meta(
            META,
            &serde_json::to_string(&paired).map_err(ApiError::internal)?,
        )?;
        Ok(())
    }

    /// Records a call of the builder; with `hello` (a poll) also what it reports. Returns
    /// whether it was offline before.
    fn contact(&self, db: &Db, hello: Option<(&Hello, Option<&str>)>) -> ApiResult<bool> {
        let timing = self.timing();
        let mut inner = self.inner.lock().unwrap();
        let was_online = inner.online(&timing);
        inner.contact = Some(Instant::now());
        if !was_online {
            // It (re)connected: check its setup again.
            inner.doctor_at = None;
        }
        let save_seen = inner
            .seen_saved
            .is_none_or(|t| t.elapsed() >= SEEN_SAVE_EVERY);
        let Some(paired) = inner.paired.as_mut() else {
            return Ok(!was_online);
        };
        let mut changed = save_seen;
        if let Some((hello, name)) = hello {
            if &paired.hello != hello {
                paired.hello = hello.clone();
                changed = true;
            }
            if let Some(name) = name.map(str::trim)
                && name != paired.name
            {
                paired.name = name.into();
                changed = true;
            }
        }
        paired.last_seen = Some(Utc::now());
        if changed {
            let snapshot = paired.clone();
            inner.seen_saved = Some(Instant::now());
            self.save(db, Some(&snapshot))?;
        }
        Ok(!was_online)
    }

    /// Records a call for task `id`: `410` when Kiwi no longer has it.
    fn touch_task(&self, id: &str) -> ApiResult<()> {
        let mut inner = self.inner.lock().unwrap();
        match inner.task(id) {
            Some(task) if !task.cancelled => {
                task.contact = Instant::now();
                Ok(())
            }
            _ => Err(ApiError::task_gone()),
        }
    }

    /// Kiwi no longer wants task `id`: a task the builder has not taken yet is dropped,
    /// a running one is cancelled at its next call.
    fn cancel_task(&self, id: &str) {
        let mut inner = self.inner.lock().unwrap();
        inner.tasks.retain_mut(|t| {
            if t.id != id {
                return true;
            }
            t.cancelled = true;
            t.done = None;
            t.started_at.is_some()
        });
    }

    /// Takes task `id` away when the builder hasn't called for it for too long; returns the
    /// builder's name then.
    fn lost(&self, id: &str) -> Option<String> {
        let lost = self.timing().lost;
        let mut inner = self.inner.lock().unwrap();
        let name = inner.name();
        let index = inner
            .tasks
            .iter()
            .position(|t| t.id == id && t.contact.elapsed() >= lost)?;
        inner.tasks.remove(index);
        Some(name)
    }

    /// Drops tasks nobody waits for any more (cancelled ones, doctor checks) whose builder
    /// went quiet.
    fn sweep(&self) {
        let lost = self.timing().lost;
        let mut inner = self.inner.lock().unwrap();
        inner
            .tasks
            .retain(|t| t.done.is_some() || t.contact.elapsed() < lost);
    }
}

/// `401 builder_unpaired` unless `token` is the paired builder's; its pairing's generation.
fn authenticate(kiwi: &Kiwi, token: &str) -> ApiResult<u64> {
    let digest = hex::encode(Sha256::digest(token.as_bytes()));
    let inner = kiwi.automation.mac.inner.lock().unwrap();
    match &inner.paired {
        Some(p) if !token.is_empty() && p.token_sha256 == digest => Ok(inner.generation),
        _ => Err(ApiError::builder_unpaired()),
    }
}

/// A builder route's caller: passes Kiwi's identity rule for the request (the owner's
/// tailnet device, or the computer itself where it is the owner's alone) without becoming a
/// device. [`BuilderAuth`] also needs the paired builder's token.
pub struct BuilderOrigin;

impl FromRequestParts<Arc<Kiwi>> for BuilderOrigin {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, kiwi: &Arc<Kiwi>) -> ApiResult<Self> {
        if !kiwi.automation.mac.supported() {
            return Err(ApiError::builder_unsupported());
        }
        let connection = parts
            .extensions
            .get::<ConnectInfo<Connection>>()
            .map(|c| &c.0);
        let forwarded_for = parts
            .headers
            .get_all("x-forwarded-for")
            .iter()
            .next_back()
            .and_then(|v| v.to_str().ok());
        let origin = peers::origin(kiwi, connection, forwarded_for).await;
        crate::devices::require_owner_origin(kiwi, origin).await?;
        Ok(Self)
    }
}

/// The paired builder: [`BuilderOrigin`] and `Authorization: Bearer <token>`. Holds the
/// generation of its pairing.
pub struct BuilderAuth(pub u64);

impl FromRequestParts<Arc<Kiwi>> for BuilderAuth {
    type Rejection = ApiError;
    async fn from_request_parts(parts: &mut Parts, kiwi: &Arc<Kiwi>) -> ApiResult<Self> {
        BuilderOrigin::from_request_parts(parts, kiwi).await?;
        let token = parts
            .headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .unwrap_or("");
        Ok(Self(authenticate(kiwi, token.trim())?))
    }
}

// ---- Kiwi Builds' side ----

/// Runs `command` of the iOS runner for `job` on the Mac builder and returns its results
/// like a local run ([`automation::process::Events::finish`]). Kiwi enforces `timeout`,
/// cancellation and the lost-contact rule; the builder hears of them at its next call.
pub(crate) async fn run_task(
    kiwi: &Kiwi,
    job: &Job,
    command: &str,
    request: &Value,
    timeout: Duration,
    token: &CancellationToken,
) -> Result<Vec<Value>, Failure> {
    let mac = &kiwi.automation.mac;
    let (sender, mut receiver) = oneshot::channel();
    {
        let mut inner = mac.inner.lock().unwrap();
        if inner.paired.is_none() || !mac.supported() {
            return Err((
                "iOS apps need a Mac builder. Pair one in Settings › Mac builder.".into(),
                true,
            ));
        }
        // A cancelled task of the same job the builder hasn't heard about yet.
        inner.tasks.retain(|t| t.id != job.id);
        inner.tasks.push(Task::new(
            job.id.clone(),
            command,
            Some(job.app.clone()),
            request.clone(),
            timeout,
            Some(job.id.clone()),
            Some(sender),
        ));
    }
    mac.wake.notify_waiters();
    let lost = mac.timing().lost;
    let mut tick = tokio::time::interval((lost / 4).min(Duration::from_secs(1)));
    let deadline = tokio::time::sleep(timeout);
    tokio::pin!(deadline);
    loop {
        tokio::select! {
            result = &mut receiver => {
                let ended = || Err(failed("The Mac builder's task ended without a result"));
                return result.unwrap_or_else(|_| ended());
            }
            _ = token.cancelled() => {
                mac.cancel_task(&job.id);
                return Err(failed("Cancelled"));
            }
            _ = &mut deadline => {
                mac.cancel_task(&job.id);
                return Err(failed("The Mac builder exceeded its time limit"));
            }
            _ = tick.tick() => {
                if let Some(name) = mac.lost(&job.id) {
                    mac.retryable.lock().unwrap().insert(job.id.clone());
                    return Err(failed(format!("Lost contact with {name} during the build.")));
                }
            }
        }
    }
}

/// The scheduler's minute: forgets tasks nobody waits for, and lets queued iOS jobs start
/// (or say what they wait for) as the builder comes and goes.
pub fn tick(kiwi: &Arc<Kiwi>) {
    let mac = &kiwi.automation.mac;
    mac.sweep();
    if kiwi.automation.remote_ios() && queued_ios(kiwi) {
        automation::start_next(kiwi);
    }
}

fn queued_ios(kiwi: &Kiwi) -> bool {
    kiwi.automation.snapshot().jobs.iter().any(|j| {
        j.finished_at.is_none()
            && j.status == "queued"
            && automation::recipe(&j.app).is_ok_and(|r| !r.android())
    })
}

// ---- owner routes ----

/// `GET /api/builder`.
pub fn view(kiwi: &Kiwi, public_url: &str) -> Value {
    let mac = &kiwi.automation.mac;
    if !mac.supported() {
        return json!({"supported": false, "reason": UNSUPPORTED, "builder": null, "pairing": null});
    }
    let state = kiwi.automation.snapshot();
    let checks = kiwi.automation.checks(&state);
    let timing = mac.timing();
    let inner = mac.inner.lock().unwrap();
    let builder = inner.paired.as_ref().map(|p| {
        let task = inner.tasks.iter().filter(|t| !t.cancelled).find_map(|t| {
            let job = state.jobs.iter().find(|j| Some(&j.id) == t.job.as_ref())?;
            let recipe = automation::recipe(&job.app).ok()?;
            Some(json!({"job": job.id, "app": job.app, "name": recipe.name,
                    "command": t.command, "stage": job.stage,
                    "started_at": t.started_at.unwrap_or(t.created_at)}))
        });
        json!({
            "name": p.name, "paired_at": p.paired_at, "last_seen": p.last_seen,
            "online": inner.online(&timing), "compatible": p.hello.compatible(),
            "version": p.hello.version, "os": p.hello.os,
            "checks": checks, "task": task,
        })
    });
    let pairing = inner
        .pairing
        .as_ref()
        .filter(|(_, expires)| *expires > Utc::now())
        .map(|(code, expires)| pairing_json(code, *expires, public_url));
    json!({"supported": true, "reason": null, "builder": builder, "pairing": pairing})
}

fn pairing_json(code: &str, expires_at: Time, public_url: &str) -> Value {
    let shown = format!("{}-{}-{}", &code[..4], &code[4..8], &code[8..]);
    json!({"code": shown, "expires_at": expires_at,
        "command": format!("./install.sh --builder --server {public_url} --pair {shown}")})
}

/// `POST /api/builder/pairing`: a new one-time code, replacing an unused one.
pub fn start_pairing(kiwi: &Kiwi, public_url: &str) -> ApiResult<Value> {
    let mac = &kiwi.automation.mac;
    if !mac.supported() {
        return Err(ApiError::builder_unsupported());
    }
    let mut random = [0u8; CODE_LENGTH];
    getrandom::fill(&mut random).map_err(ApiError::internal)?;
    // 256 is a multiple of 32: every character is equally likely.
    let code: String = random
        .iter()
        .map(|b| CODE_ALPHABET[usize::from(*b) % CODE_ALPHABET.len()] as char)
        .collect();
    let expires = Utc::now() + mac.timing().code_lifetime;
    mac.inner.lock().unwrap().pairing = Some((code.clone(), expires));
    Ok(pairing_json(&code, expires, public_url))
}

/// `DELETE /api/builder/pairing`.
pub fn withdraw_pairing(kiwi: &Kiwi) {
    kiwi.automation.mac.inner.lock().unwrap().pairing = None;
}

/// `DELETE /api/builder`: revokes the builder's token and ends its task. Queued iOS jobs
/// are cancelled: without a builder, iOS apps aren't built.
pub fn unpair(kiwi: &Kiwi) -> ApiResult<()> {
    let mac = &kiwi.automation.mac;
    {
        let mut inner = mac.inner.lock().unwrap();
        let Some(paired) = inner.paired.take() else {
            return Err(ApiError::not_found("No Mac builder is paired."));
        };
        tracing::info!("removed the Mac builder {}", paired.name);
        let message = format!("{} was removed from Kiwi during the build.", paired.name);
        inner.end_tasks(&message, &mut mac.retryable.lock().unwrap());
        inner.contact = None;
        inner.generation += 1;
        mac.save(&kiwi.db, None)?;
    }
    mac.wake.notify_waiters();
    let now = Utc::now();
    kiwi.automation.change(&kiwi.db, |s| {
        for job in s.jobs.iter_mut().filter(|j| {
            j.finished_at.is_none()
                && j.status == "queued"
                && automation::recipe(&j.app).is_ok_and(|r| !r.android())
        }) {
            job.status = "cancelled".into();
            job.stage = "Cancelled".into();
            job.error = Some(
                "The Mac builder was removed. Pair a Mac in Settings › Mac builder, then check again."
                    .into(),
            );
            job.finished_at = Some(now);
        }
        Ok(())
    })
}

/// `POST /api/builder/check`: the builder runs `doctor` at its next poll.
pub fn request_check(kiwi: &Kiwi) -> ApiResult<()> {
    let mac = &kiwi.automation.mac;
    if !mac.supported() {
        return Err(ApiError::builder_unsupported());
    }
    {
        let mut inner = mac.inner.lock().unwrap();
        if inner.paired.is_none() {
            return Err(ApiError::conflict("No Mac builder is paired."));
        }
        inner.doctor_requested = true;
    }
    mac.wake.notify_waiters();
    Ok(())
}

// ---- builder routes ----

// No `deny_unknown_fields`: serde doesn't support it with `flatten`.
#[derive(Debug, Deserialize)]
pub struct PairBody {
    pub code: String,
    pub name: String,
    #[serde(flatten)]
    pub hello: Hello,
}

/// A code as typed: case and separators don't matter.
fn normalize_code(code: &str) -> String {
    code.chars()
        .filter(|c| !matches!(c, '-' | ' '))
        .map(|c| c.to_ascii_uppercase())
        .collect()
}

/// `POST /api/builder/pair`: the code is the credential. Returns the builder's token, which
/// Kiwi keeps only as a SHA-256. A builder paired before is replaced (its token revoked,
/// its task ended).
pub fn pair(kiwi: &Kiwi, body: PairBody, public_url: &str) -> ApiResult<Value> {
    let mac = &kiwi.automation.mac;
    let mut inner = mac.inner.lock().unwrap();
    while inner
        .failures
        .front()
        .is_some_and(|t| t.elapsed() >= PAIR_WINDOW)
    {
        inner.failures.pop_front();
    }
    if inner.failures.len() >= PAIR_FAILURES {
        return Err(ApiError::rate_limited(
            "Too many wrong pairing codes. Try again in a few minutes.",
        ));
    }
    let given = normalize_code(&body.code);
    let matches = inner.pairing.as_ref().is_some_and(|(code, expires)| {
        *expires > Utc::now()
            && given.len() == code.len()
            && given
                .bytes()
                .zip(code.bytes())
                .fold(0u8, |acc, (a, b)| acc | (a ^ b))
                == 0
    });
    if !matches {
        inner.failures.push_back(Instant::now());
        return Err(ApiError::bad_code());
    }
    let name = body.name.trim().to_owned();
    if !text_ok(&name, 64) {
        return Err(ApiError::bad_request(
            "Name the Mac builder (up to 64 characters).",
        ));
    }
    body.hello.validate()?;
    let mut secret = [0u8; 32];
    getrandom::fill(&mut secret).map_err(ApiError::internal)?;
    let token = B64.encode(secret);
    let paired = Paired {
        name: name.clone(),
        token_sha256: hex::encode(Sha256::digest(token.as_bytes())),
        paired_at: Utc::now(),
        last_seen: Some(Utc::now()),
        hello: body.hello,
    };
    mac.save(&kiwi.db, Some(&paired))?;
    if let Some(old) = inner.paired.take() {
        tracing::info!("the Mac builder {name} replaces {}", old.name);
        let message = format!(
            "{} was replaced by another Mac builder during the build.",
            old.name
        );
        inner.end_tasks(&message, &mut mac.retryable.lock().unwrap());
    } else {
        tracing::info!("paired the Mac builder {name}");
    }
    inner.paired = Some(paired);
    inner.generation += 1;
    inner.pairing = None;
    inner.failures.clear();
    inner.contact = Some(Instant::now());
    inner.seen_saved = Some(Instant::now());
    inner.doctor_at = None;
    drop(inner);
    mac.wake.notify_waiters();
    Ok(json!({"token": token, "kiwi": public_url, "name": name}))
}

// No `deny_unknown_fields`: serde doesn't support it with `flatten`.
#[derive(Debug, Deserialize)]
pub struct PollBody {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(flatten)]
    pub hello: Hello,
}

/// `POST /api/builder/poll`: waits up to [`Timing::poll`] for a task.
pub async fn poll(kiwi: &Arc<Kiwi>, auth: BuilderAuth, body: PollBody) -> ApiResult<Option<Value>> {
    body.hello.validate()?;
    let name = body
        .name
        .as_deref()
        .map(str::trim)
        .filter(|n| text_ok(n, 64));
    let mac = &kiwi.automation.mac;
    mac.contact(&kiwi.db, Some((&body.hello, name)))?;
    abandon_running(kiwi);
    if kiwi.automation.remote_ios() && queued_ios(kiwi) {
        automation::start_next(kiwi);
    }
    let until = tokio::time::Instant::now() + mac.timing().poll;
    loop {
        let notified = mac.wake.notified();
        tokio::pin!(notified);
        notified.as_mut().enable();
        if let Some(task) = take_task(kiwi, auth.0)? {
            return Ok(Some(task));
        }
        tokio::select! {
            _ = &mut notified => {}
            _ = tokio::time::sleep_until(until) => break,
            _ = kiwi.cancel.cancelled() => break,
        }
    }
    mac.contact(&kiwi.db, None)?;
    Ok(None)
}

/// A builder that asks for work runs no task: the ones it had taken (it restarted) are
/// lost.
fn abandon_running(kiwi: &Kiwi) {
    let mac = &kiwi.automation.mac;
    let mut inner = mac.inner.lock().unwrap();
    let name = inner.name();
    let mut retryable = mac.retryable.lock().unwrap();
    inner.tasks.retain_mut(|t| {
        if t.started_at.is_none() {
            return true;
        }
        if let Some(job) = &t.job
            && t.done.is_some()
        {
            retryable.insert(job.clone());
        }
        t.complete(Err(failed(format!(
            "Lost contact with {name} during the build."
        ))));
        false
    });
}

/// The next task for the builder: a build job's first, else a doctor check when one is
/// due (while Kiwi Builds is on and the builder runs this Kiwi's runner).
fn take_task(kiwi: &Kiwi, generation: u64) -> ApiResult<Option<Value>> {
    let mac = &kiwi.automation.mac;
    let timing = mac.timing();
    let builds_on = kiwi.settings().kiwi_builds;
    let mut inner = mac.inner.lock().unwrap();
    if inner.paired.is_none() || inner.generation != generation {
        return Err(ApiError::builder_unpaired());
    }
    if let Some(task) = inner
        .tasks
        .iter_mut()
        .find(|t| t.started_at.is_none() && !t.cancelled && t.job.is_some())
    {
        task.started_at = Some(Utc::now());
        task.contact = Instant::now();
        return Ok(Some(task.json()));
    }
    let compatible = inner.paired.as_ref().is_some_and(|p| p.hello.compatible());
    let due = inner.doctor_requested
        || inner
            .doctor_at
            .is_none_or(|t| t.elapsed() >= timing.doctor_every);
    if !builds_on || !compatible || !due {
        return Ok(None);
    }
    inner.doctor_requested = false;
    inner.doctor_at = Some(Instant::now());
    let mut task = Task::new(
        automation::new_job_id()?,
        "doctor",
        None,
        json!({}),
        timing.doctor_timeout,
        None,
        None,
    );
    task.started_at = Some(Utc::now());
    let shown = task.json();
    inner.tasks.push(task);
    Ok(Some(shown))
}

/// `POST /api/builder/tasks/:id/events`: applies the runner's events like a local run's.
/// Returns whether the builder should stop the runner.
pub fn events(kiwi: &Kiwi, id: &str, events: Vec<Value>) -> ApiResult<bool> {
    let mac = &kiwi.automation.mac;
    mac.contact(&kiwi.db, None)?;
    let (mut collected, job) = {
        let mut inner = mac.inner.lock().unwrap();
        let task = inner.task(id).ok_or_else(ApiError::task_gone)?;
        if task.cancelled {
            return Ok(true);
        }
        task.contact = Instant::now();
        task.bytes += events
            .iter()
            .map(|e| e.to_string().len() + 1)
            .sum::<usize>();
        if task.bytes > automation::process::EVENT_LIMIT {
            task.cancelled = true;
            task.complete(Err(failed("The builder's events exceed their limit")));
            return Ok(true);
        }
        if events.is_empty() {
            return Ok(false);
        }
        let collected = task.events.take().ok_or_else(|| {
            ApiError::conflict("Send a task's events one request after the other.")
        })?;
        (collected, task.job.clone())
    };
    let mut on_stage = |stage: &str| match &job {
        Some(job) => kiwi
            .automation
            .job(&kiwi.db, job, |j| j.stage = stage.into())
            .map_err(failed),
        None => Ok(()),
    };
    let mut on_event = |_: &Value| Ok(());
    let mut artifact = None;
    let mut result = Ok(());
    for event in events {
        if event["event"] == "artifact" {
            artifact = event["sha256"]
                .as_str()
                .filter(|s| valid_sha(s))
                .map(str::to_owned);
        }
        if let Err(e) = collected.apply(kiwi, event, &mut on_stage, &mut on_event) {
            result = Err(e);
            break;
        }
    }
    let mut inner = mac.inner.lock().unwrap();
    let Some(task) = inner.task(id) else {
        return Err(ApiError::task_gone());
    };
    task.events = Some(collected);
    if artifact.is_some() {
        task.artifact = artifact;
    }
    if let Err(failure) = result {
        task.cancelled = true;
        task.complete(Err(failure));
    }
    Ok(task.cancelled)
}

/// `PUT /api/builder/tasks/:id/artifact?sha256=`: the build's IPA, streamed to
/// `automation/` while it is hashed, then moved to `automation/artifacts/<sha256>.ipa`.
pub async fn upload(
    kiwi: &Kiwi,
    id: &str,
    sha256: &str,
    length: Option<u64>,
    body: Body,
) -> ApiResult<()> {
    let mac = &kiwi.automation.mac;
    mac.contact(&kiwi.db, None)?;
    if !valid_sha(sha256) {
        return Err(ApiError::bad_request("Invalid artifact SHA-256."));
    }
    let length = length.ok_or_else(|| ApiError::bad_request("Content-Length is required."))?;
    if length > ARTIFACT_LIMIT {
        return Err(ApiError::new(
            StatusCode::PAYLOAD_TOO_LARGE,
            "bad_request",
            "The patched app is larger than 4 GiB.",
        ));
    }
    {
        let mut inner = mac.inner.lock().unwrap();
        let task = inner.task(id).ok_or_else(ApiError::task_gone)?;
        if task.cancelled {
            return Err(ApiError::task_gone());
        }
        if task.command != "build" || task.artifact.as_deref() != Some(sha256) {
            return Err(ApiError::conflict(
                "Send the artifact event with this SHA-256 before its upload.",
            ));
        }
        if task.uploading {
            return Err(ApiError::conflict("The patched app is being uploaded."));
        }
        task.uploading = true;
        task.contact = Instant::now();
    }
    let root = kiwi.config.data_dir.join("automation");
    let partial = root.join(format!("{id}.upload"));
    let result = receive(mac, id, sha256, length, body, &partial).await;
    let result = match result {
        Ok(()) => {
            let artifacts = root.join("artifacts");
            tokio::fs::create_dir_all(&artifacts)
                .await
                .map_err(ApiError::internal)
                .and(
                    tokio::fs::rename(&partial, artifacts.join(format!("{sha256}.ipa")))
                        .await
                        .map_err(ApiError::internal),
                )
        }
        Err(e) => Err(e),
    };
    if result.is_err() {
        let _ = tokio::fs::remove_file(&partial).await;
    }
    let mut inner = mac.inner.lock().unwrap();
    let Some(task) = inner.task(id) else {
        return result.and(Err(ApiError::task_gone()));
    };
    task.uploading = false;
    task.contact = Instant::now();
    if result.is_ok() {
        task.uploaded = Some(sha256.into());
    }
    result
}

async fn receive(
    mac: &MacBuilder,
    id: &str,
    sha256: &str,
    length: u64,
    body: Body,
    partial: &std::path::Path,
) -> ApiResult<()> {
    let mut file = tokio::fs::File::create(partial)
        .await
        .map_err(ApiError::internal)?;
    let mut hash = Sha256::new();
    let mut received = 0u64;
    let mut touched = Instant::now();
    let mut stream = body.into_data_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| ApiError::bad_request(format!("The upload failed: {e}")))?;
        received += chunk.len() as u64;
        if received > length {
            return Err(ApiError::bad_request(
                "The upload exceeds its Content-Length.",
            ));
        }
        hash.update(&chunk);
        file.write_all(&chunk).await.map_err(ApiError::internal)?;
        if touched.elapsed() >= Duration::from_secs(1) {
            // A long upload keeps the task alive; a cancelled task stops it.
            mac.touch_task(id)?;
            touched = Instant::now();
        }
    }
    if received != length {
        return Err(ApiError::bad_request("The upload is incomplete."));
    }
    file.sync_all().await.map_err(ApiError::internal)?;
    if hex::encode(hash.finalize()) != sha256 {
        return Err(ApiError::bad_request(
            "The uploaded app doesn't match its SHA-256.",
        ));
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FinishBody {
    pub status: String,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub provenance: Option<Value>,
}

/// `POST /api/builder/tasks/:id/finish`: ends the task and hands Kiwi Builds its result.
pub async fn finish(kiwi: &Kiwi, id: &str, body: FinishBody) -> ApiResult<()> {
    let mac = &kiwi.automation.mac;
    mac.contact(&kiwi.db, None)?;
    if !matches!(body.status.as_str(), "ok" | "failed" | "cancelled") {
        return Err(ApiError::bad_request("Unknown task status."));
    }
    let (mut task, name) = {
        let mut inner = mac.inner.lock().unwrap();
        let name = inner.name();
        let index = inner
            .tasks
            .iter()
            .position(|t| t.id == id)
            .ok_or_else(ApiError::task_gone)?;
        let task = &inner.tasks[index];
        if task.cancelled {
            inner.tasks.remove(index);
            return Err(ApiError::task_gone());
        }
        if task.uploading || task.events.is_none() {
            return Err(ApiError::conflict("The task is still receiving data."));
        }
        let build_ok = body.status == "ok" && task.command == "build";
        if build_ok && (task.uploaded.is_none() || task.uploaded != task.artifact) {
            return Err(ApiError::conflict(
                "Upload the patched app before finishing the build.",
            ));
        }
        if let Some(provenance) = body.provenance.as_ref().filter(|_| build_ok)
            && (!provenance.is_object()
                || provenance["sha256"].as_str() != task.uploaded.as_deref())
        {
            return Err(ApiError::bad_request(
                "The provenance doesn't describe the uploaded app.",
            ));
        }
        (inner.tasks.remove(index), name)
    };
    if body.status == "ok"
        && task.command == "build"
        && let (Some(provenance), Some(sha)) = (&body.provenance, &task.uploaded)
    {
        let path = kiwi
            .config
            .data_dir
            .join("automation/artifacts")
            .join(format!("{sha}.json"));
        tokio::fs::write(
            &path,
            serde_json::to_vec(provenance).map_err(ApiError::internal)?,
        )
        .await
        .map_err(ApiError::internal)?;
    }
    let events = task.events.take().unwrap_or_default();
    let result = match body.status.as_str() {
        "ok" => events.finish(true, "The Mac builder"),
        "failed" => match (events.runner_error(), body.error.as_deref()) {
            (None, Some(error)) if !error.trim().is_empty() => {
                Err(failed(error.chars().take(1024).collect::<String>()))
            }
            _ => events.finish(false, "The Mac builder"),
        },
        _ => {
            if let Some(job) = &task.job {
                mac.retryable.lock().unwrap().insert(job.clone());
            }
            Err(failed(format!("{name} stopped the build.")))
        }
    };
    task.complete(result);
    Ok(())
}
