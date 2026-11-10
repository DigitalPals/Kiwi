//! Web push: subscriptions and delivery behind [`PushTransport`] (fake in tests).

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, RwLock},
};

use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD as B64};
use chrono::Utc;
use kiwi_push::{Outcome, PushSender, Subscription, Urgency};
use serde::{Deserialize, Serialize};

use crate::{
    Kiwi,
    db::{SubscriptionRow, Time},
    devices::BoxFuture,
    error::{ApiError, ApiResult},
};

/// A push that did not get an answer from the push service. `transient` errors (network,
/// timeouts) are retried later; others mean the subscription cannot be used.
#[derive(Debug, Clone)]
pub struct SendError {
    pub message: String,
    pub transient: bool,
}

pub trait PushTransport: Send + Sync + 'static {
    fn send<'a>(
        &'a self,
        sub: &'a Subscription,
        payload: &'a [u8],
        topic: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Outcome, SendError>>;

    /// Names Kiwi's public URL as the VAPID subject from now on (it became known after the
    /// start, see [`crate::learn_public_url`]).
    fn set_subject(&self, _subject: &str) {}
}

/// Web Push with the VAPID key in the data directory.
pub struct WebPush(RwLock<Arc<PushSender>>);

impl WebPush {
    pub fn new(sender: PushSender) -> Self {
        Self(RwLock::new(Arc::new(sender)))
    }
}

/// Notifications stay useful for a day (e.g. while the phone is offline).
const TTL_SECS: u32 = 24 * 60 * 60;
/// Subscriptions are dropped after this many consecutive failures.
const MAX_FAILURES: i64 = 10;

impl PushTransport for WebPush {
    fn send<'a>(
        &'a self,
        sub: &'a Subscription,
        payload: &'a [u8],
        topic: Option<&'a str>,
    ) -> BoxFuture<'a, Result<Outcome, SendError>> {
        let sender = self.0.read().unwrap().clone();
        Box::pin(async move {
            sender
                .send(sub, payload, TTL_SECS, Urgency::Normal, topic)
                .await
                .map_err(|e| SendError {
                    transient: e.is_transient(),
                    message: e.to_string(),
                })
        })
    }

    fn set_subject(&self, subject: &str) {
        let mut sender = self.0.write().unwrap();
        match PushSender::new(sender.vapid().clone(), subject.to_owned()) {
            Ok(new) => *sender = Arc::new(new),
            Err(e) => tracing::warn!("keeping the Web Push subject: {e}"),
        }
    }
}

/// A rejection the push service will repeat: a 4xx other than 404/410 (gone), 408 (timeout)
/// and 429 (rate limited). Only these count toward deleting a subscription.
pub fn permanent_rejection(status: u16) -> bool {
    (400..500).contains(&status) && !matches!(status, 404 | 408 | 410 | 429)
}

/// Delay before the next delivery attempt of a notification after `failures` failed ones:
/// 1, 2, 4, ... minutes, at most an hour.
pub fn retry_delay(failures: u32) -> chrono::Duration {
    let minutes = 1i64 << failures.clamp(1, 7).saturating_sub(1);
    chrono::Duration::minutes(minutes.min(60))
}

/// A notice is retried for this long after its first failed delivery (the TTL of the push
/// itself), then given up.
const RETRY_FOR: chrono::Duration = chrono::Duration::days(1);
/// Given-up notices are forgotten after this long (they would be retried again).
const FORGET_AFTER: chrono::Duration = chrono::Duration::days(30);

/// A notice whose delivery failed.
struct Pending {
    failures: u32,
    first_failure: Time,
    next: Time,
}

/// Per notification (dedupe key): failed deliveries and when to try again. In memory; a
/// restart retries right away.
#[derive(Default)]
pub struct NoticeBackoff(Mutex<HashMap<String, Pending>>);

impl NoticeBackoff {
    pub fn due(&self, key: &str, now: Time) -> bool {
        self.0
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get(key)
            .is_none_or(|p| p.next <= now && now - p.first_failure < RETRY_FOR)
    }

    pub fn failed(&self, key: &str, now: Time) {
        let mut map = self.0.lock().unwrap_or_else(|e| e.into_inner());
        map.retain(|_, p| now - p.first_failure < FORGET_AFTER);
        let entry = map.entry(key.to_owned()).or_insert(Pending {
            failures: 0,
            first_failure: now,
            next: now,
        });
        entry.failures += 1;
        entry.next = now + retry_delay(entry.failures);
    }

    pub fn delivered(&self, key: &str) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).remove(key);
    }
}

/// Sends a notification once per dedupe `key`: skipped when already sent or while a failed
/// delivery backs off. Returns whether it was delivered now.
pub async fn notify_once(
    kiwi: &Kiwi,
    device_id: &str,
    key: &str,
    payload: &Payload,
) -> ApiResult<bool> {
    let now = Utc::now();
    if kiwi.db.notification_sent(key)? || !kiwi.notices.due(key, now) {
        return Ok(false);
    }
    if send_to_device(kiwi, device_id, payload).await? > 0 {
        kiwi.db.mark_notification(key, Utc::now())?;
        kiwi.notices.delivered(key);
        Ok(true)
    } else {
        kiwi.notices.failed(key, Utc::now());
        Ok(false)
    }
}

/// The JSON the service worker shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Payload {
    pub title: String,
    pub body: String,
    pub url: String,
    pub badge: usize,
    pub tag: String,
}

#[derive(Debug, Deserialize)]
pub struct SubscribeBody {
    pub endpoint: String,
    pub keys: Keys,
}

#[derive(Debug, Deserialize)]
pub struct Keys {
    pub p256dh: String,
    pub auth: String,
}

fn key_len(value: &str) -> Option<usize> {
    B64.decode(value.trim_end_matches('='))
        .ok()
        .map(|k| k.len())
}

pub fn subscribe(kiwi: &Kiwi, device_id: &str, body: SubscribeBody) -> ApiResult<()> {
    if !kiwi_push::endpoint_allowed(&body.endpoint) {
        return Err(ApiError::bad_request(
            "This push endpoint is not a known browser push service.",
        ));
    }
    if key_len(&body.keys.p256dh) != Some(65) || key_len(&body.keys.auth) != Some(16) {
        return Err(ApiError::bad_request("Invalid push subscription keys."));
    }
    kiwi.db.save_subscription(&SubscriptionRow {
        endpoint: body.endpoint,
        device_id: device_id.to_owned(),
        p256dh: body.keys.p256dh,
        auth: body.keys.auth,
        created_at: Utc::now(),
        last_success_at: None,
        failures: 0,
    })?;
    Ok(())
}

/// Sends to every subscription of a device; returns how many deliveries succeeded. Gone
/// subscriptions are removed; permanent rejections are counted (the subscription is removed
/// after 10 in a row); transient failures (network, 5xx, 408, 429) are not.
pub async fn send_to_device(kiwi: &Kiwi, device_id: &str, payload: &Payload) -> ApiResult<usize> {
    let bytes = serde_json::to_vec(payload).map_err(ApiError::internal)?;
    let mut delivered = 0;
    for row in kiwi.db.subscriptions(device_id)? {
        let sub = Subscription {
            endpoint: row.endpoint.clone(),
            p256dh: row.p256dh.clone(),
            auth: row.auth.clone(),
        };
        let outcome = kiwi.push.send(&sub, &bytes, Some(&payload.tag)).await;
        match outcome {
            Ok(Outcome::Delivered) => {
                delivered += 1;
                kiwi.db.push_delivered(&row.endpoint, Utc::now())?;
            }
            Ok(Outcome::Gone) => {
                tracing::info!("push subscription gone; removing it");
                kiwi.db.delete_subscription(&row.endpoint, None)?;
            }
            Ok(Outcome::Rejected { status, body }) if permanent_rejection(status) => {
                tracing::warn!("push rejected with HTTP {status}: {body}");
                failed(kiwi, &row.endpoint)?;
            }
            Ok(Outcome::Rejected { status, body }) => {
                tracing::warn!("push temporarily failed with HTTP {status}: {body}");
            }
            Err(e) if e.transient => tracing::warn!("push failed: {}", e.message),
            Err(e) => {
                tracing::warn!("push failed: {}", e.message);
                failed(kiwi, &row.endpoint)?;
            }
        }
    }
    Ok(delivered)
}

fn failed(kiwi: &Kiwi, endpoint: &str) -> ApiResult<()> {
    if kiwi.db.push_failed(endpoint)? >= MAX_FAILURES {
        kiwi.db.delete_subscription(endpoint, None)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejections_and_retry_delays() {
        for status in [400, 401, 403, 413] {
            assert!(permanent_rejection(status), "{status}");
        }
        for status in [404, 408, 410, 429, 500, 502, 503, 301] {
            assert!(!permanent_rejection(status), "{status}");
        }
        let minutes: Vec<i64> = (1..=9).map(|n| retry_delay(n).num_minutes()).collect();
        assert_eq!(minutes, [1, 2, 4, 8, 16, 32, 60, 60, 60]);

        let backoff = NoticeBackoff::default();
        let now = Utc::now();
        assert!(backoff.due("k", now));
        backoff.failed("k", now);
        assert!(!backoff.due("k", now + chrono::Duration::seconds(59)));
        assert!(backoff.due("k", now + chrono::Duration::minutes(1)));
        backoff.failed("k", now);
        assert!(!backoff.due("k", now + chrono::Duration::seconds(119)));
        assert!(backoff.due("other", now));
        backoff.delivered("k");
        assert!(backoff.due("k", now));

        // Retried hourly for a day after the first failure, then given up.
        let hours = chrono::Duration::hours;
        for hour in 0..23 {
            backoff.failed("k", now + hours(hour));
        }
        assert!(backoff.due("k", now + hours(23)));
        backoff.failed("k", now + hours(23));
        assert!(!backoff.due("k", now + hours(24)));
        assert!(!backoff.due("k", now + chrono::Duration::days(20)));
        // Long after, other failures forget it.
        backoff.failed("other", now + chrono::Duration::days(31));
        assert!(backoff.due("k", now + chrono::Duration::days(31)));
    }
}
