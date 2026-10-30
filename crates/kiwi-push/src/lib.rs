//! Web Push sender: VAPID keys (RFC 8292), `aes128gcm` payload encryption (RFC 8291) and
//! delivery to a push service (RFC 8030).

use std::fmt;
use std::io::{self, Write};
use std::path::Path;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use aes_gcm::Aes128Gcm;
use aes_gcm::aead::{Aead, KeyInit};
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD as B64;
use hkdf::Hkdf;
use p256::ecdsa::signature::Signer;
use p256::ecdsa::{Signature, SigningKey};
use p256::elliptic_curve::Generate;
use p256::elliptic_curve::sec1::ToSec1Point;
use p256::{PublicKey, SecretKey};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use url::Url;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("VAPID key file: {0}")]
    Io(#[from] io::Error),
    #[error("invalid VAPID key file: {0}")]
    KeyFile(String),
    #[error("invalid VAPID subject (expected https: or mailto: URI): {0}")]
    InvalidSubject(String),
    #[error("invalid push subscription: {0}")]
    InvalidSubscription(&'static str),
    #[error("push endpoint is not an allowed push service")]
    EndpointNotAllowed,
    #[error("push payload too large: {0} bytes (max {MAX_PAYLOAD})")]
    PayloadTooLarge(usize),
    #[error("random number generator failed")]
    Rng,
    #[error("push request failed: {0}")]
    Http(#[from] reqwest::Error),
}

impl Error {
    /// A network or local failure that may succeed later, as opposed to a subscription or
    /// payload the push service will never accept.
    pub fn is_transient(&self) -> bool {
        matches!(self, Self::Http(_) | Self::Rng | Self::Io(_))
    }
}

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Record size advertised in the `aes128gcm` header.
const RECORD_SIZE: u32 = 4096;
/// Header: salt (16) + rs (4) + idlen (1) + keyid (65 byte public key).
const HEADER_LEN: usize = 16 + 4 + 1 + 65;
/// Largest plaintext that keeps the body within the 4096 bytes every push service accepts:
/// 4096 - header - GCM tag (16) - padding delimiter (1).
pub const MAX_PAYLOAD: usize = 4096 - HEADER_LEN - 16 - 1;
/// VAPID JWT lifetime (RFC 8292 allows up to 24 h).
const JWT_LIFETIME: Duration = Duration::from_secs(12 * 60 * 60);
const MAX_BODY_IN_OUTCOME: usize = 512;

/// The application server's VAPID key pair (P-256).
#[derive(Clone)]
pub struct Vapid {
    key: SigningKey,
}

#[derive(Serialize, Deserialize)]
struct KeyFile {
    private_key: String,
    public_key: String,
}

impl Vapid {
    pub fn generate() -> Result<Self> {
        let key = SigningKey::try_generate().map_err(|_| Error::Rng)?;
        Ok(Self { key })
    }

    /// Loads the key pair from `path`, or creates it (mode 0600) when the file does not exist.
    pub fn load_or_create(path: &Path) -> Result<Self> {
        match std::fs::read(path) {
            Ok(bytes) => Self::from_json(&bytes),
            Err(e) if e.kind() == io::ErrorKind::NotFound => {
                let vapid = Self::generate()?;
                match vapid.persist(path) {
                    Ok(()) => Ok(vapid),
                    // Another process created it first; use theirs.
                    Err(e) if e.kind() == io::ErrorKind::AlreadyExists => {
                        Self::from_json(&std::fs::read(path)?)
                    }
                    Err(e) => Err(e.into()),
                }
            }
            Err(e) => Err(e.into()),
        }
    }

    fn from_json(bytes: &[u8]) -> Result<Self> {
        let file: KeyFile =
            serde_json::from_slice(bytes).map_err(|e| Error::KeyFile(e.to_string()))?;
        let secret = decode_b64(&file.private_key)
            .ok_or_else(|| Error::KeyFile("private_key is not base64url".into()))?;
        let key = SigningKey::from_slice(&secret)
            .map_err(|_| Error::KeyFile("private_key is not a P-256 scalar".into()))?;
        let vapid = Self { key };
        if vapid.public_key() != file.public_key.trim_end_matches('=') {
            return Err(Error::KeyFile(
                "public_key does not match private_key".into(),
            ));
        }
        Ok(vapid)
    }

    fn persist(&self, path: &Path) -> io::Result<()> {
        let dir = path.parent().filter(|d| !d.as_os_str().is_empty());
        let dir = dir.unwrap_or(Path::new("."));
        std::fs::create_dir_all(dir)?;
        let json = serde_json::to_vec_pretty(&KeyFile {
            private_key: B64.encode(self.key.to_bytes()),
            public_key: self.public_key(),
        })?;
        let mut builder = tempfile::Builder::new();
        builder.prefix(".vapid-").suffix(".tmp");
        #[cfg(unix)]
        builder.permissions(std::os::unix::fs::PermissionsExt::from_mode(0o600));
        let mut tmp = builder.tempfile_in(dir)?;
        tmp.write_all(&json)?;
        tmp.as_file().sync_all()?;
        tmp.persist_noclobber(path).map_err(|e| e.error)?;
        Ok(())
    }

    /// The public key as base64url (no padding) of the 65-byte uncompressed point, as used for
    /// `applicationServerKey` in `PushManager.subscribe()`.
    pub fn public_key(&self) -> String {
        B64.encode(self.public_key_bytes())
    }

    fn public_key_bytes(&self) -> Vec<u8> {
        self.key
            .verifying_key()
            .to_sec1_point(false)
            .as_bytes()
            .to_vec()
    }

    /// Builds the RFC 8292 `Authorization` header value for an endpoint origin.
    fn authorization(&self, audience: &str, subject: &str, now: SystemTime) -> String {
        let exp = (now + JWT_LIFETIME)
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let header = B64.encode(br#"{"typ":"JWT","alg":"ES256"}"#);
        let claims = serde_json::json!({ "aud": audience, "exp": exp, "sub": subject });
        let signing_input = format!("{header}.{}", B64.encode(claims.to_string()));
        let signature: Signature = self.key.sign(signing_input.as_bytes());
        let jwt = format!("{signing_input}.{}", B64.encode(signature.to_bytes()));
        format!("vapid t={jwt}, k={}", self.public_key())
    }
}

impl fmt::Debug for Vapid {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Vapid")
            .field("public_key", &self.public_key())
            .finish_non_exhaustive()
    }
}

/// A `PushSubscription` as sent by the browser (`keys.p256dh`, `keys.auth`, base64url).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Subscription {
    pub endpoint: String,
    pub p256dh: String,
    pub auth: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Urgency {
    VeryLow,
    Low,
    #[default]
    Normal,
    High,
}

impl Urgency {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::VeryLow => "very-low",
            Self::Low => "low",
            Self::Normal => "normal",
            Self::High => "high",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Delivered,
    /// 404/410: the subscription expired or was removed; delete it.
    Gone,
    Rejected {
        status: u16,
        body: String,
    },
}

pub struct PushSender {
    vapid: Vapid,
    subject: String,
    client: reqwest::Client,
}

impl PushSender {
    /// `subject` is the VAPID `sub` claim: an `https:` URL or a `mailto:` address.
    pub fn new(vapid: Vapid, subject: String) -> Result<Self> {
        let valid = match Url::parse(&subject) {
            Ok(url) => match url.scheme() {
                "https" => url.host().is_some(),
                "mailto" => url.path().contains('@'),
                _ => false,
            },
            Err(_) => false,
        };
        if !valid {
            return Err(Error::InvalidSubject(subject));
        }
        let client = reqwest::Client::builder()
            .redirect(reqwest::redirect::Policy::none())
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            vapid,
            subject,
            client,
        })
    }

    pub fn vapid(&self) -> &Vapid {
        &self.vapid
    }

    /// Encrypts `payload` for the subscription and posts it to its push service.
    /// The endpoint must pass [`endpoint_allowed`]. An invalid `topic` (RFC 8030 allows at most
    /// 32 base64url characters) is replaced by a stable hash of it.
    pub async fn send(
        &self,
        sub: &Subscription,
        payload: &[u8],
        ttl_secs: u32,
        urgency: Urgency,
        topic: Option<&str>,
    ) -> Result<Outcome> {
        if !endpoint_allowed(&sub.endpoint) {
            return Err(Error::EndpointNotAllowed);
        }
        self.deliver(sub, payload, ttl_secs, urgency, topic).await
    }

    async fn deliver(
        &self,
        sub: &Subscription,
        payload: &[u8],
        ttl_secs: u32,
        urgency: Urgency,
        topic: Option<&str>,
    ) -> Result<Outcome> {
        let endpoint =
            Url::parse(&sub.endpoint).map_err(|_| Error::InvalidSubscription("endpoint"))?;
        let ua_public =
            decode_b64(&sub.p256dh).ok_or(Error::InvalidSubscription("p256dh encoding"))?;
        let auth = decode_b64(&sub.auth).ok_or(Error::InvalidSubscription("auth encoding"))?;
        let mut salt = [0u8; 16];
        getrandom::fill(&mut salt).map_err(|_| Error::Rng)?;
        let ephemeral = SecretKey::try_generate().map_err(|_| Error::Rng)?;
        let body = encrypt(&ua_public, &auth, payload, &ephemeral, salt)?;

        let audience = endpoint.origin().ascii_serialization();
        let mut request = self
            .client
            .post(endpoint)
            .header(
                "Authorization",
                self.vapid
                    .authorization(&audience, &self.subject, SystemTime::now()),
            )
            .header("TTL", ttl_secs.to_string())
            .header("Urgency", urgency.as_str())
            .header("Content-Encoding", "aes128gcm")
            .header("Content-Type", "application/octet-stream")
            .body(body);
        if let Some(topic) = topic {
            request = request.header("Topic", push_topic(topic));
        }
        let response = request.send().await?;
        let status = response.status();
        Ok(match status.as_u16() {
            _ if status.is_success() => Outcome::Delivered,
            404 | 410 => Outcome::Gone,
            code => Outcome::Rejected {
                status: code,
                body: rejection_body(response).await,
            },
        })
    }
}

/// Read only the diagnostic prefix, plus one byte to detect truncation. A large or
/// endless error response must not delay notification retries or be buffered in memory.
async fn rejection_body(mut response: reqwest::Response) -> String {
    let limit = MAX_BODY_IN_OUTCOME + 1;
    let mut bytes = Vec::with_capacity(limit);
    while bytes.len() < limit {
        match response.chunk().await {
            Ok(Some(chunk)) => {
                let count = chunk.len().min(limit - bytes.len());
                bytes.extend_from_slice(&chunk[..count]);
            }
            Ok(None) => break,
            Err(_) => return String::new(),
        }
    }
    truncate(
        String::from_utf8_lossy(&bytes).into_owned(),
        MAX_BODY_IN_OUTCOME,
    )
}

impl fmt::Debug for PushSender {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("PushSender")
            .field("vapid", &self.vapid)
            .field("subject", &self.subject)
            .finish_non_exhaustive()
    }
}

/// True for `https` endpoints on a known browser push service (Apple, Google FCM, Mozilla,
/// Microsoft WNS) on the default port and without credentials.
pub fn endpoint_allowed(endpoint: &str) -> bool {
    let Ok(url) = Url::parse(endpoint) else {
        return false;
    };
    if url.scheme() != "https"
        || url.port().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return false;
    }
    let Some(url::Host::Domain(host)) = url.host() else {
        return false;
    };
    matches!(
        host,
        "web.push.apple.com" | "fcm.googleapis.com" | "updates.push.services.mozilla.com"
    ) || [
        ".push.apple.com",
        ".push.services.mozilla.com",
        ".notify.windows.com",
    ]
    .iter()
    .any(|suffix| host.ends_with(suffix))
}

/// RFC 8291 `aes128gcm` encryption of a single record with the given ephemeral key and salt.
fn encrypt(
    ua_public: &[u8],
    auth_secret: &[u8],
    payload: &[u8],
    as_secret: &SecretKey,
    salt: [u8; 16],
) -> Result<Vec<u8>> {
    if payload.len() > MAX_PAYLOAD {
        return Err(Error::PayloadTooLarge(payload.len()));
    }
    if auth_secret.len() != 16 {
        return Err(Error::InvalidSubscription("auth must be 16 bytes"));
    }
    if ua_public.len() != 65 || ua_public[0] != 0x04 {
        return Err(Error::InvalidSubscription(
            "p256dh must be an uncompressed P-256 point",
        ));
    }
    let ua_key = PublicKey::from_sec1_bytes(ua_public)
        .map_err(|_| Error::InvalidSubscription("p256dh is not a P-256 point"))?;
    let as_public = as_secret.public_key().to_sec1_point(false);
    let as_public = as_public.as_bytes();
    let shared = as_secret.diffie_hellman(&ua_key);

    // IKM = HKDF(auth_secret, ecdh_secret, "WebPush: info" || 0x00 || ua_public || as_public)
    let key_info = [b"WebPush: info\0".as_slice(), ua_public, as_public].concat();
    let mut ikm = [0u8; 32];
    Hkdf::<Sha256>::new(Some(auth_secret), shared.raw_secret_bytes())
        .expand(&key_info, &mut ikm)
        .expect("32 bytes is a valid HKDF-SHA-256 length");
    let prk = Hkdf::<Sha256>::new(Some(&salt), &ikm);
    let mut cek = [0u8; 16];
    let mut nonce = [0u8; 12];
    prk.expand(b"Content-Encoding: aes128gcm\0", &mut cek)
        .expect("valid HKDF length");
    prk.expand(b"Content-Encoding: nonce\0", &mut nonce)
        .expect("valid HKDF length");

    let plaintext = [payload, &[2]].concat();
    let ciphertext = Aes128Gcm::new_from_slice(&cek)
        .expect("16-byte key")
        .encrypt(&nonce.into(), plaintext.as_slice())
        .expect("payload within AES-GCM limits");

    let mut body = Vec::with_capacity(HEADER_LEN + ciphertext.len());
    body.extend_from_slice(&salt);
    body.extend_from_slice(&RECORD_SIZE.to_be_bytes());
    body.push(as_public.len() as u8);
    body.extend_from_slice(as_public);
    body.extend_from_slice(&ciphertext);
    Ok(body)
}

/// Decodes base64url or standard base64, with or without padding.
fn decode_b64(s: &str) -> Option<Vec<u8>> {
    let normalized: String = s
        .trim()
        .trim_end_matches('=')
        .chars()
        .map(|c| match c {
            '+' => '-',
            '/' => '_',
            c => c,
        })
        .collect();
    B64.decode(normalized).ok()
}

fn push_topic(topic: &str) -> String {
    let valid = !topic.is_empty()
        && topic.len() <= 32
        && topic
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if valid {
        topic.to_owned()
    } else {
        B64.encode(Sha256::digest(topic.as_bytes()))[..32].to_owned()
    }
}

fn truncate(mut s: String, max: usize) -> String {
    if s.len() > max {
        let mut end = max;
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        s.truncate(end);
        s.push('…');
    }
    s
}

#[cfg(test)]
mod tests;
