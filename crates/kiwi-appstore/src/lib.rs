//! A small App Store Connect API client for Kiwi's family devices: it registers iPhones in
//! the developer account and creates development provisioning profiles that include them.
//! It authenticates with a team API key (`AuthKey_<id>.p8`) and ES256 tokens.

mod key;

use std::time::{Duration, SystemTime};

use base64::Engine;
use base64::engine::general_purpose::STANDARD;
use p256::ecdsa::SigningKey;
use reqwest::{Method, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sha1::{Digest, Sha1};

pub use key::{TOKEN_LIFETIME, parse_private_key, token};

pub const DEFAULT_URL: &str = "https://api.appstoreconnect.apple.com";
/// Devices per device class (iPhone, iPad, ...) a membership may register each year.
pub const DEVICE_LIMIT: usize = 100;
const TIMEOUT: Duration = Duration::from_secs(60);
const PAGE: usize = 200;
const MAX_PAGES: usize = 20;

pub type Result<T, E = Error> = std::result::Result<T, E>;

/// Messages are shown to the user as is.
#[derive(Debug, thiserror::Error)]
pub enum Error {
    /// The saved credentials cannot be used.
    #[error("{0}")]
    Key(String),
    /// App Store Connect answered with an error.
    #[error("{message}")]
    Api { status: u16, message: String },
    #[error("App Store Connect could not be reached: {0}")]
    Network(String),
    #[error("Unexpected answer from App Store Connect: {0}")]
    Response(String),
}

impl Error {
    /// The key was refused (revoked, wrong issuer or not allowed to manage devices).
    pub fn is_auth(&self) -> bool {
        matches!(
            self,
            Self::Api {
                status: 401 | 403,
                ..
            }
        )
    }
}

/// A team API key: issuer id, key id and the `.p8` file's text.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Credentials {
    pub issuer_id: String,
    pub key_id: String,
    pub private_key: String,
}

impl std::fmt::Debug for Credentials {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Credentials")
            .field("issuer_id", &self.issuer_id)
            .field("key_id", &self.key_id)
            .finish_non_exhaustive()
    }
}

impl Credentials {
    /// Trims the fields and checks their form (not whether Apple accepts them).
    pub fn normalized(&self) -> Result<Self> {
        let issuer_id = self.issuer_id.trim().to_lowercase();
        let key_id = self.key_id.trim().to_uppercase();
        let uuid = issuer_id.len() == 36
            && issuer_id.char_indices().all(|(i, c)| match i {
                8 | 13 | 18 | 23 => c == '-',
                _ => c.is_ascii_hexdigit(),
            });
        if !uuid {
            return Err(Error::Key(
                "The issuer ID is a UUID like 57246542-96fe-1a63-e053-0824d011072a.".into(),
            ));
        }
        if !(8..=16).contains(&key_id.len()) || !key_id.chars().all(|c| c.is_ascii_alphanumeric()) {
            return Err(Error::Key(
                "The key ID has 10 letters and digits, like 2X9R4HXF34.".into(),
            ));
        }
        parse_private_key(&self.private_key)?;
        Ok(Self {
            issuer_id,
            key_id,
            private_key: self.private_key.trim().to_owned(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub udid: String,
    /// "IOS", "MAC_OS", ...
    pub platform: String,
    /// "ENABLED" or "DISABLED".
    pub status: String,
    /// "IPHONE", "IPAD", "APPLE_WATCH", ...
    pub device_class: Option<String>,
    pub model: Option<String>,
}

impl Device {
    pub fn enabled(&self) -> bool {
        self.status == "ENABLED"
    }
    pub fn is_ios(&self) -> bool {
        self.platform == "IOS"
    }
    pub fn has_udid(&self, udid: &str) -> bool {
        self.udid.eq_ignore_ascii_case(udid)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BundleId {
    pub id: String,
    /// "*" for the team wildcard, else e.g. "com.example.app".
    pub identifier: String,
    pub name: String,
    pub platform: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Certificate {
    pub id: String,
    pub name: String,
    pub certificate_type: String,
    /// Upper-case hex SHA-1 of the certificate (as the keychain identity reports it).
    pub sha1: Option<String>,
}

/// A profile App Store Connect just created.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewProfile {
    pub id: String,
    pub uuid: String,
    pub name: String,
    /// The `.mobileprovision` file.
    pub content: Vec<u8>,
}

pub struct Client {
    http: reqwest::Client,
    base: String,
    issuer_id: String,
    key_id: String,
    key: SigningKey,
}

fn text(v: &Value, key: &str) -> Option<String> {
    v.get(key).and_then(Value::as_str).map(str::to_owned)
}

fn id(item: &Value) -> Result<String> {
    text(item, "id").ok_or_else(|| Error::Response("an item has no id".into()))
}

fn attributes(item: &Value) -> &Value {
    &item["attributes"]
}

fn parse_device(item: &Value) -> Result<Device> {
    let a = attributes(item);
    Ok(Device {
        id: id(item)?,
        name: text(a, "name").unwrap_or_default(),
        udid: text(a, "udid").ok_or_else(|| Error::Response("a device has no UDID".into()))?,
        platform: text(a, "platform").unwrap_or_default(),
        status: text(a, "status").unwrap_or_default(),
        device_class: text(a, "deviceClass"),
        model: text(a, "model"),
    })
}

fn parse_bundle_id(item: &Value) -> Result<BundleId> {
    let a = attributes(item);
    Ok(BundleId {
        id: id(item)?,
        identifier: text(a, "identifier").unwrap_or_default(),
        name: text(a, "name").unwrap_or_default(),
        platform: text(a, "platform"),
    })
}

fn parse_certificate(item: &Value) -> Result<Certificate> {
    let a = attributes(item);
    let sha1 = text(a, "certificateContent")
        .and_then(|b64| STANDARD.decode(b64.trim()).ok())
        .map(|der| hex::encode_upper(Sha1::digest(der)));
    Ok(Certificate {
        id: id(item)?,
        name: text(a, "name").unwrap_or_default(),
        certificate_type: text(a, "certificateType").unwrap_or_default(),
        sha1,
    })
}

fn parse_new_profile(item: &Value) -> Result<NewProfile> {
    let a = attributes(item);
    let content = text(a, "profileContent")
        .and_then(|b64| STANDARD.decode(b64.trim()).ok())
        .ok_or_else(|| Error::Response("the new profile has no content".into()))?;
    Ok(NewProfile {
        id: id(item)?,
        uuid: text(a, "uuid").unwrap_or_default(),
        name: text(a, "name").unwrap_or_default(),
        content,
    })
}

/// The readable part of an App Store Connect error response.
fn api_error(status: StatusCode, body: &[u8]) -> Error {
    let parsed: Option<Value> = serde_json::from_slice(body).ok();
    let message = parsed
        .as_ref()
        .and_then(|v| v["errors"].as_array())
        .map(|errors| {
            errors
                .iter()
                .filter_map(|e| text(e, "detail").or_else(|| text(e, "title")))
                .collect::<Vec<_>>()
                .join(" ")
        })
        .filter(|m| !m.is_empty());
    let message = match (status.as_u16(), message) {
        (401, _) => "App Store Connect did not accept the API key. Check the issuer ID, key ID and .p8 file, and that the key was not revoked.".to_owned(),
        (403, Some(m)) => format!("The API key may not do this ({m}). Use a key with the Admin role."),
        (403, None) => "The API key may not do this. Use a key with the Admin role.".to_owned(),
        (_, Some(m)) => m,
        (code, None) => format!("App Store Connect answered HTTP {code}."),
    };
    Error::Api {
        status: status.as_u16(),
        message,
    }
}

impl Client {
    pub fn new(credentials: &Credentials, base_url: &str) -> Result<Self> {
        let key = parse_private_key(&credentials.private_key)?;
        let http = reqwest::Client::builder()
            .timeout(TIMEOUT)
            .connect_timeout(Duration::from_secs(10))
            .user_agent(concat!("Kiwi/", env!("CARGO_PKG_VERSION")))
            .build()
            .map_err(|e| Error::Network(e.to_string()))?;
        Ok(Self {
            http,
            base: base_url.trim_end_matches('/').to_owned(),
            issuer_id: credentials.issuer_id.clone(),
            key_id: credentials.key_id.clone(),
            key,
        })
    }

    async fn send(&self, method: Method, url: &str, body: Option<Value>) -> Result<Value> {
        let jwt = token(&self.key, &self.key_id, &self.issuer_id, SystemTime::now());
        let mut request = self.http.request(method, url).bearer_auth(jwt);
        if let Some(body) = body {
            request = request.json(&body);
        }
        let response = request
            .send()
            .await
            .map_err(|e| Error::Network(e.without_url().to_string()))?;
        let status = response.status();
        let bytes = response
            .bytes()
            .await
            .map_err(|e| Error::Network(e.without_url().to_string()))?;
        if !status.is_success() {
            return Err(api_error(status, &bytes));
        }
        if bytes.is_empty() {
            return Ok(Value::Null);
        }
        serde_json::from_slice(&bytes).map_err(|e| Error::Response(e.to_string()))
    }

    fn url(&self, path: &str, query: &[(&str, &str)]) -> String {
        let mut url = format!("{}{path}", self.base);
        if !query.is_empty() {
            let encoded: String = url::form_urlencoded::Serializer::new(String::new())
                .extend_pairs(query)
                .finish();
            url = format!("{url}?{encoded}");
        }
        url
    }

    /// Every item of a list, following `links.next` (only to the same server).
    async fn list(&self, path: &str, query: &[(&str, &str)]) -> Result<Vec<Value>> {
        let limit = PAGE.to_string();
        let mut query = query.to_vec();
        query.push(("limit", &limit));
        let mut next = Some(self.url(path, &query));
        let mut items = Vec::new();
        for _ in 0..MAX_PAGES {
            let Some(url) = next.take() else { break };
            let page = self.send(Method::GET, &url, None).await?;
            items.extend(page["data"].as_array().cloned().unwrap_or_default());
            next = text(&page["links"], "next").filter(|n| n.starts_with(&self.base));
        }
        Ok(items)
    }

    /// Every device of the team.
    pub async fn devices(&self) -> Result<Vec<Device>> {
        self.list("/v1/devices", &[])
            .await?
            .iter()
            .map(parse_device)
            .collect()
    }

    pub async fn register_device(&self, name: &str, udid: &str) -> Result<Device> {
        let body = json!({ "data": { "type": "devices", "attributes": {
            "name": name, "udid": udid, "platform": "IOS",
        }}});
        let reply = self
            .send(Method::POST, &self.url("/v1/devices", &[]), Some(body))
            .await?;
        parse_device(&reply["data"])
    }

    pub async fn enable_device(&self, id: &str) -> Result<Device> {
        let body =
            json!({ "data": { "type": "devices", "id": id, "attributes": { "status": "ENABLED" }}});
        let path = format!("/v1/devices/{id}");
        let reply = self
            .send(Method::PATCH, &self.url(&path, &[]), Some(body))
            .await?;
        parse_device(&reply["data"])
    }

    /// The App ID with exactly this identifier ("*" is the team wildcard).
    pub async fn bundle_id(&self, identifier: &str) -> Result<Option<BundleId>> {
        let items = self
            .list("/v1/bundleIds", &[("filter[identifier]", identifier)])
            .await?;
        for item in &items {
            let bundle = parse_bundle_id(item)?;
            if bundle.identifier == identifier {
                return Ok(Some(bundle));
            }
        }
        Ok(None)
    }

    pub async fn create_bundle_id(&self, identifier: &str, name: &str) -> Result<BundleId> {
        let body = json!({ "data": { "type": "bundleIds", "attributes": {
            "identifier": identifier, "name": name, "platform": "IOS",
        }}});
        let reply = self
            .send(Method::POST, &self.url("/v1/bundleIds", &[]), Some(body))
            .await?;
        parse_bundle_id(&reply["data"])
    }

    /// Apple Development (and older iOS Development) certificates.
    pub async fn development_certificates(&self) -> Result<Vec<Certificate>> {
        self.list(
            "/v1/certificates",
            &[("filter[certificateType]", "DEVELOPMENT,IOS_DEVELOPMENT")],
        )
        .await?
        .iter()
        .map(parse_certificate)
        .collect()
    }

    /// Creates an iOS development profile for an App ID, certificates and devices
    /// (resource ids).
    pub async fn create_profile(
        &self,
        name: &str,
        bundle_id: &str,
        certificates: &[String],
        devices: &[String],
    ) -> Result<NewProfile> {
        let refs = |kind: &str, ids: &[String]| -> Vec<Value> {
            ids.iter()
                .map(|id| json!({ "type": kind, "id": id }))
                .collect()
        };
        let body = json!({ "data": {
            "type": "profiles",
            "attributes": { "name": name, "profileType": "IOS_APP_DEVELOPMENT" },
            "relationships": {
                "bundleId": { "data": { "type": "bundleIds", "id": bundle_id } },
                "certificates": { "data": refs("certificates", certificates) },
                "devices": { "data": refs("devices", devices) },
            },
        }});
        let reply = self
            .send(Method::POST, &self.url("/v1/profiles", &[]), Some(body))
            .await?;
        parse_new_profile(&reply["data"])
    }

    pub async fn delete_profile(&self, id: &str) -> Result<()> {
        let path = format!("/v1/profiles/{id}");
        self.send(Method::DELETE, &self.url(&path, &[]), None)
            .await
            .map(|_| ())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn credentials() -> Credentials {
        Credentials {
            issuer_id: " 57246542-96FE-1A63-E053-0824D011072A ".into(),
            key_id: "2x9r4hxf34".into(),
            private_key: key::tests::p8(),
        }
    }

    #[test]
    fn credentials_are_checked_and_normalized() {
        let c = credentials().normalized().unwrap();
        assert_eq!(c.issuer_id, "57246542-96fe-1a63-e053-0824d011072a");
        assert_eq!(c.key_id, "2X9R4HXF34");
        assert!(!format!("{c:?}").contains("PRIVATE"));
        let mut bad = credentials();
        bad.issuer_id = "nope".into();
        assert!(
            bad.normalized()
                .unwrap_err()
                .to_string()
                .contains("issuer ID")
        );
        let mut bad = credentials();
        bad.key_id = "x".into();
        assert!(bad.normalized().unwrap_err().to_string().contains("key ID"));
        let mut bad = credentials();
        bad.private_key = "-----BEGIN PRIVATE KEY-----".into();
        assert!(matches!(bad.normalized(), Err(Error::Key(_))));
    }

    #[test]
    fn parses_items_and_errors() {
        let device = parse_device(&json!({ "type": "devices", "id": "D1", "attributes": {
            "name": "Noah's iPhone", "udid": "00008110-001A2B3C4D5E801E", "platform": "IOS",
            "status": "ENABLED", "deviceClass": "IPHONE", "model": "iPhone 15" }}))
        .unwrap();
        assert!(device.enabled() && device.is_ios());
        assert!(device.has_udid("00008110-001a2b3c4d5e801e"));
        assert!(parse_device(&json!({ "id": "x", "attributes": {} })).is_err());

        let der = b"certificate".to_vec();
        let cert = parse_certificate(&json!({ "id": "C1", "attributes": {
            "name": "Apple Development: Alex", "certificateType": "DEVELOPMENT",
            "certificateContent": STANDARD.encode(&der) }}))
        .unwrap();
        assert_eq!(cert.sha1.unwrap(), hex::encode_upper(Sha1::digest(&der)));

        let profile = parse_new_profile(&json!({ "id": "P1", "attributes": {
            "name": "Kiwi *", "uuid": "abc", "profileContent": STANDARD.encode(b"plist") }}))
        .unwrap();
        assert_eq!(profile.content, b"plist");

        let conflict = api_error(
            StatusCode::CONFLICT,
            br#"{"errors":[{"status":"409","code":"ENTITY_ERROR","title":"An attribute value is invalid.","detail":"A device with number '1' already exists on this team."}]}"#,
        );
        assert_eq!(
            conflict.to_string(),
            "A device with number '1' already exists on this team."
        );
        assert!(api_error(StatusCode::UNAUTHORIZED, b"").is_auth());
        assert!(
            api_error(StatusCode::FORBIDDEN, b"{}")
                .to_string()
                .contains("Admin role")
        );
        assert_eq!(
            api_error(StatusCode::BAD_GATEWAY, b"<html>").to_string(),
            "App Store Connect answered HTTP 502."
        );
    }

    /// Answers each request with the next canned (status, body), made from the server's
    /// base URL, and records the requests.
    async fn server(
        answers: impl FnOnce(&str) -> Vec<(u16, String)>,
    ) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let answers = answers(&base);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let log = seen.clone();
        tokio::spawn(async move {
            for (status, body) in answers {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut request = Vec::new();
                let mut buf = [0u8; 4096];
                loop {
                    let n = socket.read(&mut buf).await.unwrap();
                    request.extend_from_slice(&buf[..n]);
                    let text = String::from_utf8_lossy(&request).to_string();
                    if let Some(end) = text.find("\r\n\r\n") {
                        let length = text
                            .lines()
                            .find_map(|l| {
                                l.to_ascii_lowercase()
                                    .strip_prefix("content-length:")
                                    .map(|v| v.trim().parse::<usize>().unwrap())
                            })
                            .unwrap_or(0);
                        if request.len() >= end + 4 + length {
                            break;
                        }
                    }
                    if n == 0 {
                        break;
                    }
                }
                log.lock()
                    .unwrap()
                    .push(String::from_utf8_lossy(&request).to_string());
                let reply = format!(
                    "HTTP/1.1 {status} X\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                    body.len()
                );
                socket.write_all(reply.as_bytes()).await.unwrap();
            }
        });
        (base, seen)
    }

    fn device_item(id: &str, udid: &str) -> Value {
        json!({ "type": "devices", "id": id, "attributes": {
            "name": id, "udid": udid, "platform": "IOS", "status": "ENABLED" }})
    }

    #[tokio::test]
    async fn follows_next_links_on_the_same_server_only() {
        let (base, seen) = server(|base| {
            vec![
                (
                    200,
                    json!({ "data": [device_item("D1", "A")],
                            "links": { "next": format!("{base}/v1/devices?cursor=2&limit=200") } })
                    .to_string(),
                ),
                (
                    200,
                    json!({ "data": [device_item("D2", "B")],
                            "links": { "next": "https://elsewhere.example/v1/devices?cursor=3" } })
                    .to_string(),
                ),
            ]
        })
        .await;
        let client = Client::new(&credentials().normalized().unwrap(), &base).unwrap();
        let devices = client.devices().await.unwrap();
        let ids: Vec<&str> = devices.iter().map(|d| d.id.as_str()).collect();
        assert_eq!(ids, ["D1", "D2"]);
        let requests = seen.lock().unwrap().clone();
        assert_eq!(requests.len(), 2);
        assert!(requests[0].starts_with("GET /v1/devices?limit=200 "));
        assert!(requests[1].starts_with("GET /v1/devices?cursor=2&limit=200 "));
        assert!(
            requests[0]
                .to_lowercase()
                .contains("authorization: bearer ")
        );
    }

    #[tokio::test]
    async fn creates_profiles_with_relationships() {
        let (base, seen) = server(|_| {
            vec![
                (
                    201,
                    json!({ "data": { "id": "P1", "attributes": {
                        "name": "Kiwi *", "uuid": "U1", "profileContent": STANDARD.encode(b"x") }}})
                    .to_string(),
                ),
                (
                    409,
                    json!({ "errors": [{ "detail": "Duplicate." }] }).to_string(),
                ),
            ]
        })
        .await;
        let client = Client::new(&credentials().normalized().unwrap(), &base).unwrap();
        let created = client
            .create_profile("Kiwi *", "B1", &["C1".into()], &["D1".into(), "D2".into()])
            .await
            .unwrap();
        assert_eq!(
            (created.uuid.as_str(), created.content.as_slice()),
            ("U1", &b"x"[..])
        );
        let err = client
            .register_device("Noah's iPhone", "A")
            .await
            .unwrap_err();
        assert_eq!(err.to_string(), "Duplicate.");
        let requests = seen.lock().unwrap().clone();
        let body: Value =
            serde_json::from_str(requests[0].split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert_eq!(
            body["data"]["attributes"]["profileType"],
            "IOS_APP_DEVELOPMENT"
        );
        assert_eq!(
            body["data"]["relationships"]["bundleId"]["data"]["id"],
            "B1"
        );
        assert_eq!(
            body["data"]["relationships"]["devices"]["data"][1]["id"],
            "D2"
        );
        assert!(requests[1].starts_with("POST /v1/devices "));
    }
}
