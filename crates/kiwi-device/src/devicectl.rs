use std::ffi::OsString;
use std::path::Path;
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::{Error, Result, process};

const XCRUN: &str = "/usr/bin/xcrun";
const PROGRAM: &str = "devicectl";
const VERSION_TIMEOUT: Duration = Duration::from_secs(15);
const LIST_TIMEOUT: Duration = Duration::from_secs(30);
const APPS_TIMEOUT: Duration = Duration::from_secs(60);
const INSTALL_TIMEOUT: Duration = Duration::from_secs(600);
/// Extra time after devicectl's own `--timeout` before the process is killed.
const KILL_GRACE: Duration = Duration::from_secs(10);
/// Seconds between the Unix epoch and the Apple reference date (2001-01-01T00:00:00Z).
const APPLE_EPOCH: f64 = 978_307_200.0;

/// A device known to CoreDevice (physical or simulator).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CoreDevice {
    /// CoreDevice identifier, accepted by `--device`.
    pub identifier: String,
    pub name: String,
    pub udid: Option<String>,
    /// Marketing name, e.g. "iPhone 17 Pro".
    pub model: Option<String>,
    /// Hardware identifier, e.g. "iPhone18,1".
    pub product_type: Option<String>,
    pub platform: Option<String>,
    pub os_version: Option<String>,
    pub physical: bool,
    /// "paired", "unpaired", ...
    pub pairing_state: Option<String>,
    /// "connected", "disconnected", "unavailable", ...
    pub tunnel_state: Option<String>,
    /// "wired", "localNetwork", "sameMachine", ...
    pub transport: Option<String>,
    pub last_connection: Option<DateTime<Utc>>,
}

impl CoreDevice {
    pub fn paired(&self) -> bool {
        self.pairing_state.as_deref() == Some("paired")
    }

    /// Paired and either connected or connectable on demand (tunnel not `unavailable`).
    pub fn reachable(&self) -> bool {
        self.paired()
            && self
                .tunnel_state
                .as_deref()
                .is_some_and(|s| s != "unavailable")
    }
}

/// An app installed on a device.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceApp {
    pub bundle_id: String,
    pub name: Option<String>,
    /// `CFBundleShortVersionString`
    pub version: Option<String>,
    /// `CFBundleVersion`
    pub build: Option<String>,
    pub removable: Option<bool>,
    /// Bundle path on the device.
    pub path: Option<String>,
}

/// True on macOS when `xcrun devicectl` runs (Xcode installed and selected).
pub async fn devicectl_available() -> bool {
    cfg!(target_os = "macos")
        && process::run(Path::new(XCRUN), &[PROGRAM, "--version"], VERSION_TIMEOUT)
            .await
            .is_ok_and(|o| o.status.success())
}

/// `devicectl list devices`: physical devices and simulators.
pub async fn list_devices() -> Result<Vec<CoreDevice>> {
    devices_from(&devicectl(["list", "devices"], LIST_TIMEOUT).await?)
}

/// `devicectl device info apps`: developer and removable apps on a device.
pub async fn list_apps(device: &str) -> Result<Vec<DeviceApp>> {
    let args = ["device", "info", "apps", "--device", device];
    apps_from(&devicectl(args, APPS_TIMEOUT).await?)
}

/// `devicectl device install app` with a `.app` bundle or an `.ipa`.
pub async fn install_app(device: &str, app_or_ipa: &Path) -> Result<()> {
    let args = ["device", "install", "app", "--device", device].map(OsString::from);
    let args = args.into_iter().chain([app_or_ipa.as_os_str().to_owned()]);
    devicectl(args, INSTALL_TIMEOUT).await.map(drop)
}

/// Parses the JSON written by `devicectl list devices --json-output`.
pub fn parse_devices(json: &[u8]) -> Result<Vec<CoreDevice>> {
    devices_from(&result_of(from_slice(json)?)?)
}

/// Parses the JSON written by `devicectl device info apps --json-output`.
pub fn parse_apps(json: &[u8]) -> Result<Vec<DeviceApp>> {
    apps_from(&result_of(from_slice(json)?)?)
}

async fn devicectl<I, S>(args: I, timeout: Duration) -> Result<Value>
where
    I: IntoIterator<Item = S>,
    S: Into<OsString>,
{
    if !cfg!(target_os = "macos") {
        return Err(Error::Unsupported);
    }
    let args = std::iter::once(PROGRAM.into()).chain(args.into_iter().map(Into::into));
    run_json(Path::new(XCRUN), args.collect(), timeout).await
}

/// Runs a devicectl-style command with `--quiet --timeout <s> --json-output <tmp>` appended
/// and returns the checked `result` object.
async fn run_json(program: &Path, mut args: Vec<OsString>, timeout: Duration) -> Result<Value> {
    let dir = tempfile::Builder::new()
        .prefix("kiwi-devicectl-")
        .tempdir()
        .map_err(|source| Error::Spawn {
            program: PROGRAM.into(),
            source,
        })?;
    let out = dir.path().join("output.json");
    let secs = timeout.as_secs().max(1).to_string();
    args.extend(["--quiet", "--timeout", &secs, "--json-output"].map(OsString::from));
    args.push(out.clone().into());
    let output = process::run(program, &args, timeout + KILL_GRACE).await?;
    match std::fs::read(&out)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
    {
        Some(json) => result_of(json),
        None if output.status.success() => Err(parse_error("no JSON output")),
        None => Err(Error::Failed {
            program: PROGRAM.into(),
            message: process::failure_message(&output),
        }),
    }
}

fn from_slice(json: &[u8]) -> Result<Value> {
    serde_json::from_slice(json).map_err(|e| parse_error(e.to_string()))
}

fn parse_error(message: impl Into<String>) -> Error {
    Error::Parse {
        program: PROGRAM.into(),
        message: message.into(),
    }
}

/// Returns `result` when `info.outcome` is "success", otherwise the reported error.
fn result_of(mut json: Value) -> Result<Value> {
    let outcome = json.pointer("/info/outcome").and_then(Value::as_str);
    if outcome == Some("success") {
        return Ok(json["result"].take());
    }
    let error = &json["error"];
    let description = str_at(error, "/userInfo/NSLocalizedDescription/string")
        .or_else(|| str_at(error, "/userInfo/NSLocalizedDescription"));
    let reason = str_at(error, "/userInfo/NSLocalizedFailureReason/string");
    let message = match (description, reason) {
        (Some(d), Some(r)) => format!("{d} {r}"),
        (Some(d), None) => d,
        _ => str_at(&json, "/errorSignature")
            .unwrap_or_else(|| format!("outcome {}", outcome.unwrap_or("unknown"))),
    };
    Err(Error::Failed {
        program: PROGRAM.into(),
        message,
    })
}

fn devices_from(result: &Value) -> Result<Vec<CoreDevice>> {
    let devices = result["devices"]
        .as_array()
        .ok_or_else(|| parse_error("missing result.devices"))?;
    Ok(devices.iter().filter_map(device_from).collect())
}

/// Reads the `properties` dictionary (jsonVersion 5) and falls back to the deprecated
/// `hardwareProperties`/`deviceProperties`/`connectionProperties`.
fn device_from(v: &Value) -> Option<CoreDevice> {
    let identifier = str_at(v, "/identifier")?;
    let s = |new: &str, old: &str| str_at(v, new).or_else(|| str_at(v, old));
    let reality = s(
        "/properties/hardware/reality",
        "/hardwareProperties/reality",
    );
    let physical = match reality {
        Some(reality) => reality != "simulated",
        None => ["/visibilityClass", "/properties/state/visibilityClass"]
            .iter()
            .all(|p| v.pointer(p).and_then(Value::as_str) != Some("simulators")),
    };
    Some(CoreDevice {
        name: s("/properties/state/name", "/deviceProperties/name")
            .unwrap_or_else(|| identifier.clone()),
        udid: s("/properties/hardware/udid", "/hardwareProperties/udid"),
        model: s(
            "/properties/hardware/marketingName",
            "/hardwareProperties/marketingName",
        ),
        product_type: s(
            "/properties/hardware/productType",
            "/hardwareProperties/productType",
        ),
        platform: s(
            "/properties/hardware/platform",
            "/hardwareProperties/platform",
        ),
        os_version: version_at(v, "/properties/software/osVersionNumber")
            .or_else(|| version_at(v, "/deviceProperties/osVersionNumber")),
        physical,
        pairing_state: s(
            "/properties/connection/pairingState",
            "/connectionProperties/pairingState",
        ),
        tunnel_state: str_at(v, "/properties/connection/tunnelState")
            .or_else(|| str_at(v, "/properties/connection/state"))
            .or_else(|| str_at(v, "/connectionProperties/tunnelState")),
        transport: s(
            "/properties/connection/transportType",
            "/connectionProperties/transportType",
        ),
        last_connection: date_at(v, "/properties/connection/lastConnectionDate")
            .or_else(|| date_at(v, "/connectionProperties/lastConnectionDate")),
        identifier,
    })
}

fn apps_from(result: &Value) -> Result<Vec<DeviceApp>> {
    let apps = result["apps"]
        .as_array()
        .ok_or_else(|| parse_error("missing result.apps"))?;
    Ok(apps
        .iter()
        .filter_map(|app| {
            Some(DeviceApp {
                bundle_id: str_at(app, "/bundleIdentifier")?,
                name: str_at(app, "/name"),
                version: str_at(app, "/version"),
                build: str_at(app, "/bundleVersion"),
                removable: app["removable"].as_bool(),
                path: str_at(app, "/url").map(file_url_path),
            })
        })
        .collect())
}

fn str_at(v: &Value, pointer: &str) -> Option<String> {
    let s = v.pointer(pointer)?.as_str()?.trim();
    (!s.is_empty()).then(|| s.to_owned())
}

/// A version is a string ("27.0") or an object with `stringValue`/`components`.
fn version_at(v: &Value, pointer: &str) -> Option<String> {
    let v = v.pointer(pointer)?;
    if let Some(s) = str_at(v, "").or_else(|| str_at(v, "/stringValue")) {
        return Some(s);
    }
    let components: Vec<u64> = v["components"]
        .as_array()?
        .iter()
        .map(Value::as_u64)
        .collect::<Option<_>>()?;
    let count = v["originalComponentsCount"]
        .as_u64()
        .unwrap_or(3)
        .clamp(1, 5) as usize;
    let parts: Vec<String> = components.iter().take(count).map(u64::to_string).collect();
    (!parts.is_empty()).then(|| parts.join("."))
}

/// Dates are RFC 3339 strings (deprecated fields) or seconds since 2001-01-01 (`properties`).
fn date_at(v: &Value, pointer: &str) -> Option<DateTime<Utc>> {
    match v.pointer(pointer)? {
        Value::Number(n) => {
            let millis = ((n.as_f64()? + APPLE_EPOCH) * 1000.0).round();
            DateTime::from_timestamp_millis(millis as i64)
        }
        Value::String(s) => DateTime::parse_from_rfc3339(s)
            .ok()
            .map(|d| d.with_timezone(&Utc)),
        _ => None,
    }
}

fn file_url_path(url: String) -> String {
    url::Url::parse(&url)
        .ok()
        .filter(|u| u.scheme() == "file")
        .and_then(|u| u.to_file_path().ok())
        .map(|p| p.to_string_lossy().trim_end_matches('/').to_owned())
        .unwrap_or(url)
}

#[cfg(all(test, unix))]
mod tests {
    use std::path::PathBuf;

    use super::*;

    /// A fake devicectl: echoes its arguments and writes JSON depending on `$1`.
    fn fake_devicectl(dir: &Path) -> PathBuf {
        let path = dir.join("fake-devicectl");
        let script = r#"#!/bin/sh
for a; do out=$a; done
echo "$@" > "$(dirname "$0")/args"
case "$1" in
  ok) printf '%s' '{"info":{"outcome":"success","jsonVersion":5},"result":{"apps":[{"bundleIdentifier":"a.b"}]}}' > "$out" ;;
  fail) printf '%s' '{"error":{"code":1000,"userInfo":{"NSLocalizedDescription":{"string":"The specified device was not found."}}},"info":{"outcome":"failed"}}' > "$out"
        echo "ERROR: The specified device was not found." >&2; exit 1 ;;
  nojson) echo "xcrun: error: unable to find utility" >&2; exit 72 ;;
  silent) exit 0 ;;
esac
"#;
        std::fs::write(&path, script).unwrap();
        path
    }

    async fn run(dir: &Path, mode: &str) -> Result<Value> {
        let program = fake_devicectl(dir);
        // Read the fixture through its interpreter. Executing a newly written script
        // can race another parallel test's fork inheriting its writable descriptor
        // before exec closes it, producing a transient ETXTBSY on Linux.
        run_json(
            Path::new("/bin/sh"),
            vec![program.into_os_string(), mode.into(), "x".into()],
            Duration::from_secs(5),
        )
        .await
    }

    #[tokio::test]
    async fn run_json_outcomes() {
        let dir = tempfile::tempdir().unwrap();
        let result = run(dir.path(), "ok").await.unwrap();
        assert_eq!(apps_from(&result).unwrap()[0].bundle_id, "a.b");
        let args = std::fs::read_to_string(dir.path().join("args")).unwrap();
        assert!(
            args.starts_with("ok x --quiet --timeout 5 --json-output /"),
            "{args}"
        );
        assert!(args.trim_end().ends_with("/output.json"), "{args}");

        let err = run(dir.path(), "fail").await.unwrap_err();
        assert!(
            matches!(&err, Error::Failed { message, .. } if message == "The specified device was not found."),
            "{err}"
        );
        let err = run(dir.path(), "nojson").await.unwrap_err();
        assert!(
            matches!(&err, Error::Failed { message, .. } if message.contains("unable to find utility")),
            "{err}"
        );
        assert!(matches!(
            run(dir.path(), "silent").await,
            Err(Error::Parse { .. })
        ));
    }

    #[tokio::test]
    async fn process_timeout_kills() {
        let start = std::time::Instant::now();
        let err = process::run(Path::new("/bin/sleep"), &["5"], Duration::from_millis(200))
            .await
            .unwrap_err();
        assert!(matches!(err, Error::Timeout { ref program, .. } if program == "sleep"));
        assert!(start.elapsed() < Duration::from_secs(2));
        let err = process::run(
            Path::new("/nonexistent/xcrun"),
            &["x"],
            Duration::from_secs(1),
        )
        .await
        .unwrap_err();
        assert!(matches!(err, Error::Spawn { .. }));
    }

    #[test]
    fn versions_and_dates() {
        let v = serde_json::json!({
            "a": {"components": [27, 0, 1, 0, 0], "originalComponentsCount": 3},
            "b": {"components": [26, 4, 0, 0, 0]},
            "c": "18.6.2",
            "d": 814947338,
            "e": "2026-10-29T06:15:38.000Z",
            "f": 814947338.25,
        });
        assert_eq!(version_at(&v, "/a").as_deref(), Some("27.0.1"));
        assert_eq!(version_at(&v, "/b").as_deref(), Some("26.4.0"));
        assert_eq!(version_at(&v, "/c").as_deref(), Some("18.6.2"));
        assert_eq!(version_at(&v, "/missing"), None);
        assert_eq!(date_at(&v, "/d"), date_at(&v, "/e"));
        assert_eq!(
            date_at(&v, "/e").unwrap().to_rfc3339(),
            "2026-10-29T06:15:38+00:00"
        );
        assert_eq!(date_at(&v, "/f").unwrap().timestamp_subsec_millis(), 250);
    }
}
