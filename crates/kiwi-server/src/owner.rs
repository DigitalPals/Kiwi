//! The owner (architecture.md "The owner"): the Tailscale user who owns the Mac, or
//! `--owner`. Kiwi is theirs alone; other people on the tailnet get `403 not_owner`.

use std::{
    sync::{Mutex, RwLock},
    time::{Duration, Instant},
};

use crate::{
    Kiwi,
    db::DeviceRow,
    error::{ApiError, ApiResult},
};

/// `tailscale status` is not asked again for the owner sooner than this.
const OWNER_RETRY: Duration = Duration::from_secs(30);

/// Who owns Kiwi, in memory.
pub struct Owner {
    /// `--owner`; when set, Tailscale is not asked.
    configured: Option<String>,
    login: RwLock<Option<String>>,
    checked: Mutex<Option<Instant>>,
}

impl Owner {
    pub fn new(configured: Option<String>, saved: Option<String>) -> Self {
        Self {
            login: RwLock::new(configured.clone().or(saved)),
            configured,
            checked: Mutex::default(),
        }
    }

    /// The owner's Tailscale login, when known.
    pub fn login(&self) -> Option<String> {
        self.login.read().unwrap().clone()
    }
}

/// Asks Tailscale who owns the Mac (at most every 30 s unless `force`), unless `--owner`
/// names them. The answer is saved, so Kiwi still knows the owner while Tailscale is down.
pub async fn refresh(kiwi: &Kiwi, force: bool) {
    if kiwi.owner.configured.is_some() {
        return;
    }
    {
        let mut checked = kiwi.owner.checked.lock().unwrap();
        if !force && checked.is_some_and(|at| at.elapsed() < OWNER_RETRY) {
            return;
        }
        *checked = Some(Instant::now());
    }
    match kiwi.devices.owner().await {
        Ok(Some(user)) => {
            if kiwi.owner.login().as_ref() != Some(&user.login) {
                tracing::info!(
                    "the Mac belongs to {} in Tailscale: they own Kiwi",
                    user.login
                );
                if let Err(e) = kiwi.db.set_meta("owner_login", &user.login) {
                    tracing::warn!("could not save the owner: {e}");
                }
                *kiwi.owner.login.write().unwrap() = Some(user.login);
            }
        }
        Ok(None) => tracing::warn!(
            "the Mac belongs to no Tailscale user (a tagged machine?): start Kiwi with --owner <login>"
        ),
        Err(e) => tracing::warn!("could not ask Tailscale who owns the Mac: {e}"),
    }
}

/// Scheduler work: asks for the owner while unknown.
pub async fn tick(kiwi: &Kiwi) {
    if kiwi.owner.login().is_none() {
        refresh(kiwi, false).await;
    }
}

/// Browser devices on the Mac and devices from before Kiwi recorded Tailscale users count
/// as the owner's; every other device must belong to the owner's Tailscale user.
pub fn is_owner(kiwi: &Kiwi, device: &DeviceRow) -> bool {
    match &device.person_id {
        None => true,
        Some(user) => kiwi.owner.login().as_ref() == Some(user),
    }
}

/// `403 not_owner` for a device of someone else (also while the owner is unknown).
pub fn require(kiwi: &Kiwi, device: &DeviceRow) -> ApiResult<()> {
    if is_owner(kiwi, device) {
        return Ok(());
    }
    Err(ApiError::not_owner(match kiwi.owner.login() {
        Some(owner) => format!(
            "This Kiwi belongs to {owner}. To use it here, sign in to Tailscale as {owner} on this device."
        ),
        None => "Kiwi doesn't know who owns this Mac yet. If the Mac is tagged in Tailscale, \
                 install Kiwi again with ./install.sh --owner <your Tailscale login>."
            .into(),
    }))
}
