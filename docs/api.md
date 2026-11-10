# Kiwi HTTP API

Contract between `crates/kiwi-server` and `web/`. JSON uses snake_case. Timestamps are
RFC 3339 strings in UTC. All `/api` responses are `application/json` with
`Cache-Control: no-store`.

## Conventions

- Mutating requests (`POST`, `PUT`, `PATCH`, `DELETE`) must send `X-Kiwi: 1`; requests with
  a body must send `Content-Type: application/json`. Otherwise `403 csrf`. The web app
  sends `{}` for POST/PATCH without parameters and no body for DELETE (it still sends
  both headers); the server accepts empty and `{}` bodies alike.
- Errors: HTTP 4xx/5xx with `{ "error": { "code": string, "message": string } }`.
  `message` is user-readable. Codes used: `bad_request`, `csrf`, `not_found`,
  `conflict`, `source_fetch`, `source_parse`, `signing_unavailable`, `app_store` (502,
  the App Store could not be reached), `device_unidentified` (503), `bad_host` (421),
  `not_owner` (403, the device belongs to someone other than the owner), `builds_off` (409,
  Kiwi Builds is turned off in Settings), `needs_mac` (409, an iOS app in Kiwi Builds on a
  Kiwi that doesn't run on macOS), `internal`.
- Every request (`/api`, `/ota` and the web app) must be addressed to the public URL's
  host (`Host`, as `tailscale serve` forwards it) or to the listen address
  (`127.0.0.1:<port>`, `localhost:<port>`, `[::1]:<port>`); an `X-Forwarded-Host`, when
  present, must be the public host. Otherwise `421 bad_host` (blocks DNS rebinding).
- The device is resolved from Tailscale (`X-Forwarded-For` + whois) or the
  `kiwi_device` cookie. When a request proxied from the tailnet cannot be identified
  because the Tailscale whois fails (retried once), the response is
  `503 device_unidentified` ("Couldn't identify this device through Tailscale. Try
  again."); the cookie is used only without a forwarded address or when whois reports that
  the address is not a peer. The web app should send requests with
  `credentials: "same-origin"`, and on startup completes `GET /api/me` (which sets the
  cookie) before sending other requests, so concurrent first requests do not each create
  a browser device.
- Path parameters are URL-encoded. `bundle_id` contains dots; it is one path segment.
- Only the owner may use Kiwi (see "The owner" below): every endpoint but `/api/health`
  answers other people's devices `403 not_owner`.
- Kiwi's Android app identifies itself with the User-Agent
  `Kiwi-Android/<versionName> (<versionCode>; Android <release>; SDK <int>; <manufacturer> <model>)`
  and is otherwise a client like the web app (same headers, same device: see "Android
  devices").

## Kiwi Builds (automation)

Kiwi patches and signs App Store apps on the Mac from the recipes in
`automation/recipes.json`: `youtube`, `youtube-music`, `twitch`, `x` and `reddit`. It
patches Android apps from the recipes in `automation/android.json`: `android-youtube`,
`android-youtube-music` and `android-reddit`. `:app` below is one of those ids; an unknown
id returns 404. Responses contain no credentials, signing keys, models, raw Apple or
Google responses, file paths or private logs.

Kiwi Builds is off until the owner turns it on (`Settings.kiwi_builds`). While it is off,
every `/api/automation` endpoint answers `409 builds_off` and nothing is built; turning it
off cancels queued and running builds. Releases already published stay in the Kiwi Builds
source. A Kiwi that had added apps before this setting existed keeps it on.

`GET /api/automation` returns `{apps, checks}`: every app's state (as below, with only its
newest 3 `jobs`) in display order, and the shared Mac setup `checks` (`{name,ready}[]`).

iOS apps are built only on a Mac. On a Kiwi that doesn't run on macOS, `checks` is always
`[{"name":"macOS","ready":false}]`, `POST .../add` and `POST .../check` of an iOS app answer
`409 needs_mac`, and added iOS apps are not scheduled. Android apps build everywhere.

`GET /api/automation/:app` returns one app's state with up to 30 `jobs`, newest first:

- From the recipe: `id`, `platform` (`"ios"` or `"android"`), `name`, `bundle_id` (for
  Android the package the patched app installs as), `icon_url` (App Store artwork or null),
  `tweak` (name and version, e.g. `"NeoFreeBird 7.3.0"`), `summary`, `experimental`,
  `warning` (null or a risk notice to show before adding), `names` (Home Screen names to
  choose from, empty when there is no choice), `app_version` (the App Store version the
  tweak supports, or null when the newest version is built), `limitations`
  (`{icon,title,detail}[]`), `included` and `available_tweaks` (`{id,name,description}[]`:
  always-built patches worth naming, and the optional ones).
- `added`: whether the app is in Kiwi Builds. Only added apps are scheduled or checked.
- `preferences`, `last_checked_at`, `last_success_key`, `last_success_tweaks`,
  `last_success_name`, `failed_recipe`, `failed_attempts` (the retry limit persists
  independently of history and resets after a successful publication) and `sequence` (the
  last Kiwi revision handed out; never reused, also after removing and adding the app).
- `published`: `{version,build,revision,date,tweaks,name}` or null. `tweaks` may be null for
  an older release whose selections cannot be recovered.
- `source_enabled`: boolean, or null before the built-in source exists. Source visibility
  is independent of the preference `enabled`.
- `configuration_changed`: true when saved patch choices or the app name differ from the
  published build. Schedule changes alone never require rebuilding.
- `next_check_at`: scheduled UTC time, or null when not added, disabled, a job is active or
  the app can't be built here (an iOS app on a Kiwi that doesn't run on macOS).
- `checks`: the shared Mac setup checks. They concern iOS builds only: an Android job
  never changes them and is never blocked by them.

Preferences, for `PATCH /api/automation/:app` (replaces them) and `POST .../add`:

```json
{"enabled":true,"interval_minutes":360,"tweaks":["rymd"],"name":"Twitter"}
```

The interval must be an integer from 15 through 1440; tweaks must be the app's optional
ids. `name` is only accepted for apps with `names` (omitted, it is the first one). Unknown
values or fields return 400. Preferences apply to the next job.

`POST /api/automation/:app/add` saves the preferences, adds the app and queues its first
build (202). `DELETE /api/automation/:app` removes it: its jobs are cancelled and it leaves
the source; installed copies keep working until they expire. Both return the app's state.

`POST /api/automation/:app/check` accepts `{}` or `{"force":true}` and returns 202. It
returns 400 for an app that is not added and 409 when the app already has a queued or
running job. `force` rebuilds an already published recipe and bypasses its three-failure
retry limit. Manual checks work when automatic checks are disabled.
`POST /api/automation/:app/cancel` returns 200: a queued job is cancelled at once; a
running one completes asynchronously and preserves the published release.

One job runs at a time. Job statuses are `queued` (waiting for another app's job; its
`stage` names it), `running`, `ready`, `unchanged`, `blocked`, `failed` or `cancelled`. Each
job has `id`, `app`, `sequence`, `stage`, `created_at`, nullable `finished_at`, `version`,
`external_id`, `recipe_key`, `signed_build_id`, a bounded user-facing `error`, and the
`preferences` it was started with; changing preferences never changes a queued or running
job. Poll while `finished_at` is null. A `ready` job has passed package and signing checks;
it does not imply a test on a physical device. An Android job's `external_id` is the
original app's version code. Its runner signs the app itself, so `signed_build_id` names
the build Kiwi creates, already `ready`, when it publishes the app, and the job ends at
stage "Available in Kiwi".

Published builds use source `kiwi-youtube` (its id predates the other apps) with each
app's original bundle id (an Android app's `bundle_id` is its patched package), and the
usual app/prepare/install APIs. Its source URL is an
internal identifier; refreshing it returns the saved local catalogue without fetching the
network. Raw artifacts and provenance files have no HTTP download route. See
[kiwi-builds.md](kiwi-builds.md) for Mac setup and operational details.

## The owner

Kiwi is one person's: the **owner**, the Tailscale user who owns the Mac (according to
tailscaled), or `--owner`. Kiwi remembers the owner (meta `owner_login`), so they are still
known while Tailscale is down. Devices whose Tailscale user is the owner, browser devices on
the Mac itself and devices seen before Kiwi recorded users are the owner's.

Every other device gets `403 not_owner` from every endpoint but `/api/health`, with a message
that names the owner ("This Kiwi belongs to alex@example.com. To use it here, sign in to
Tailscale as alex@example.com on this device."). While Kiwi doesn't know the owner (a tagged
Mac without `--owner`), that includes every device on the tailnet; the message says to install
Kiwi again with `./install.sh --owner <your Tailscale login>`. The web app shows the message
on a page of its own.

## Types

```ts
type Timestamp = string;

type Platform = "ios" | "android";

interface Device {
  id: string;                       // "ts:<StableID>" | "local:<random>"
  name: string;                     // user-editable; defaults to Tailscale name
  kind: "tailscale" | "browser";
  tailscale_name: string | null;    // "iphone194"
  platform: Platform;               // what the device installs (see "Device platforms")
  platform_source: "chosen" | "detected" | "assumed";
  detected_platform: Platform | null; // what Kiwi takes without a choice; null: nothing tells
                                    // (desktop browsers)
  client: "web" | "android_app";    // this request's client; other devices are "web"
  os_version: string | null;        // "27.0.1"; Android: "15", or null (Chrome doesn't tell)
  os_version_source: "devicectl" | "installer" | "manual" | "android_app" | "safari" | "android" | null;
                                    // highest priority first; "android_app": Kiwi's Android app
  model: string | null;             // "iPhone19,4"; "Android" for an Android device
  devicectl: LinkedCoreDevice | null;
  last_seen: Timestamp;
}

interface LinkedCoreDevice {
  id: string;                       // devicectl identifier
  name: string;
  reachable: boolean;               // paired + tunnel connected/connectable at last check
  last_sync_at: Timestamp | null;
  last_error: string | null;
}

interface CoreDevice {              // a device known to devicectl on the Mac
  id: string; name: string; udid: string | null; model: string | null;
  platform: string | null; os_version: string | null; physical: boolean;
  pairing_state: string | null; tunnel_state: string | null; transport: string | null;
  linked_device_id: string | null;  // Kiwi device linked to it
}

interface Warning {
  level: "info" | "warning" | "error";
  code: string;                     // e.g. "no_identity", "no_profile", "expiring", "source_errors", "android_app"
  message: string;
  link: string | null;              // in-app route to fix it, e.g. "/settings", "/sources"
}

interface Me {
  device: Device;
  onboarded: boolean;               // the first-run setup was completed on this device
  server: {
    version: string;
    public_url: string;             // "https://mac-studio.example.ts.net"
    team_id: string | null;
    devicectl_available: boolean;
  };
  host: {                           // the computer Kiwi runs on
    os: "macos" | "linux" | "other";
    ios_signing: "keychain" | "files"; // macOS: the login keychain; elsewhere uploaded files
    devicectl: boolean;             // devicectl is available (as server.devicectl_available)
  };
  features: Feature[];              // what to show on this device
  android_app: AndroidApp | null;   // Kiwi's Android app, when this Kiwi serves it
  push: { vapid_public_key: string; subscribed: boolean };  // subscribed: this device has >= 1 subscription
  counts: { updates: number; installed: number; sources: number; source_errors: number; preparing: number };
  warnings: Warning[];
}

// What to show on a device. ios_apps and android_apps follow Device.platform and exclude
// each other.
type Feature =
  | "ios_apps"          // iOS installs, AltStore sources, apps from links, signing, expiry timers
  | "android_apps"      // Android installs (Kiwi Builds' APKs)
  | "home_screen_app"   // the web app on an iOS device: Add to Home Screen and Web Push
  | "web_push"          // the web app (it can use Web Push: the iOS Home Screen app, Android
                        // and desktop browsers); not Kiwi's Android app
  | "android_app_offer" // the web app on an Android device, while android_app is not null
  | "devicectl";        // an iOS device on a macOS host: linking it to a paired CoreDevice

interface AndroidApp {
  package: string;                  // "io.github.digitalpals.kiwi"
  version_name: string;
  version_code: number;
  size: number;                     // bytes
  install_url: string;              // "<public_url>/android/kiwi.apk"
}

interface Source {
  id: string;
  kind: "feed" | "ipa" | "local_build"; // ipa: an IPA link or download page; local_build: Kiwi Builds
  url: string;                      // the feed, the IPA link or the download page
  ipa_url: string | null;           // ipa: the IPA its app comes from now
  name: string;
  subtitle: string | null;
  icon_url: string | null;
  enabled: boolean;
  app_count: number;
  browse: "all" | "picked";         // which apps Browse shows by default (see below)
  shown_count: number;              // apps Browse shows
  new_count: number;                // apps added since switching to "picked", not chosen yet
  added_at: Timestamp;
  last_attempt_at: Timestamp | null;
  last_success_at: Timestamp | null;
  error: string | null;
  diagnostics: string[];            // non-fatal parse warnings
}

// Browse shows an app unless it was hidden by hand. With "picked", it shows only apps turned
// on by hand; apps a source adds later stay hidden and are `is_new` until turned on or off.
// Hiding only affects Browse: installed apps still show in My Apps and get updates.
// An ipa source has one app, read from the IPA (name, version, icon). On refresh Kiwi looks
// for a new IPA link on its download page and asks the IPA's server (HEAD: ETag,
// Last-Modified, Content-Length) whether the file changed; only then is it downloaded again.
// Icons taken from IPAs have `icon_url` `/api/icons/<sha256>.png`.
// The built-in local_build source cannot be deleted. Its enabled switch controls source
// visibility; automation scheduling is managed separately through the automation API.

interface Release {
  version: string;
  build: string | null;
  revision?: number;               // local patch revision; Apple version/build remain intact
  date: string | null;
  notes: string | null;             // "what's new", plain text
  size: number | null;              // bytes
  min_os: string | null;
  max_os: string | null;
  compatible: boolean;              // for this device's OS version (true when unknown)
  incompatibility: string | null;
}

type Action = "get" | "preparing" | "install" | "installing" | "open" | "update" | "retry" | "unavailable";

interface Installed {
  version: string;
  build: string | null;
  revision?: number;               // retained from the installed Kiwi release
  signed_bundle_id: string;
  origin: "kiwi" | "device";
  verified: boolean;                // seen on the phone by devicectl
  installed_at: Timestamp;
  expires_at: Timestamp | null;     // signature/profile expiry for Kiwi builds
}

interface AppState {
  action: Action;
  update_reason: "new_version" | "resign" | null;   // when action is "update"
  installed: Installed | null;      // this app's install (same source_id and bundle_id)
  target: Release | null;           // release that Get/Update would install
  build: Build | null;              // build of the target release, if any
  install_url: string | null;       // when a ready build exists for target: an itms-services
                                    // URL, or for an Android app the https URL of its APK
  open_url: string | null;          // "kiwi-xxxxxxxxxxxx://" when installed with Kiwi's scheme
  replaces: Replaces | null;        // installing this app replaces another source's app
  foreign_install: boolean;         // Android: the package is installed with another signature
                                    // (reported by Kiwi's Android app), so Kiwi's build can't be
                                    // installed until it is removed ("Remove the other <name> first")
}

// Several sources often ship different apps with the same original bundle id (YouTubePlus,
// YouMod and uYou all use com.google.ios.youtube), so they share Kiwi's signed bundle id.
// An app's installed/open/update state only comes from an install with its own source_id and
// bundle_id. When the signed id is installed from another source (or a removed one), the app
// is treated as not installed (action "get", or "preparing"/"install"/"installing"/"retry"
// for its own builds), is never an update, and `replaces` names what installing it replaces.
// The web app shows "Replaces <name> (<source_name>)".
interface Replaces {
  name: string;                     // the installed app's name
  source_name: string;              // its source, or "Removed source"
  version: string;                  // installed version
}

interface AppSummary {
  source_id: string;
  source_name: string;
  bundle_id: string;                // original bundle id from the source
  platform: "ios" | "android";      // "android": an APK, installed as the package `bundle_id`
  name: string;
  developer: string | null;
  subtitle: string | null;
  icon_url: string | null;          // remote URL from the source
  tint_color: string | null;        // "#RRGGBB"
  category: string | null;
  unavailable: string | null;
  orphaned: boolean;                // installed from a source that was removed
  hidden: boolean;                  // not shown in Browse
  is_new: boolean;                  // added to a "picked" source since, not chosen yet
  original: Original | null;        // the App Store app it is based on, when known
  state: AppState;
}

interface StoreApp {                // an app in the App Store storefront chosen in Settings
  bundle_id: string;
  name: string;                     // "Spotify: Music and Podcasts" (in the storefront's language)
  short_name: string;               // "Spotify": the name before ": ", " - ", " – ", " — " or " | "
  developer: string | null;
  version: string;                  // current App Store version
  released_at: Timestamp | null;    // release of that version
  icon_url: string | null;
  store_url: string;                // apps.apple.com page
}

// Patched apps usually keep the official app's bundle id, so the App Store app with the same
// bundle id is their original unless another one (or none) was chosen by hand.
interface Original extends StoreApp {
  app_version: string | null;       // compared version: the signed IPA's once built, else the release's
  status: "same" | "behind" | "ahead" | "unknown";  // this app vs the App Store; "behind": the
}                                   // App Store is newer; "unknown": not comparable numerically

interface OtherVersion {            // the same app in another enabled source
  source_id: string;
  source_name: string;
  bundle_id: string;
  name: string;
  icon_url: string | null;
  tint_color: string | null;
  version: string | null;           // target release
  installed: boolean;
  hidden: boolean;
}

interface AppDetail extends AppSummary {
  description: string | null;
  screenshots: string[];
  releases: Release[];              // newest first
  permissions: { entitlements: string[]; privacy: { key: string; description: string }[] };
  dropped_entitlements: string[];   // requested entitlements the current profile cannot grant
  signed_bundle_id: string;         // what Kiwi will install it as (Android: `bundle_id`)
  source_url: string;
  original_match: "bundle_id" | "linked" | "not_found" | "none" | "pending";
                                    // how `original` was found: same bundle id, chosen by hand, no
                                    // App Store app (or the linked one is gone), "not based on an
                                    // App Store app", or not looked up yet
  other_versions: OtherVersion[];   // apps with the same original (or, without one, the same bundle id)
}

interface Build {
  id: string;
  source_id: string;
  bundle_id: string;
  version: string;
  build: string | null;
  revision?: number;
  status: "queued" | "downloading" | "signing" | "packaging" | "ready" | "failed";
  step: string | null;              // human-readable, e.g. "Signing 12 of 23"
  progress: number | null;          // 0..1 when known
  error: string | null;
  signed_bundle_id: string;
  size: number | null;              // signed IPA (or APK) bytes
  expires_at: Timestamp | null;     // null for an Android build: nothing expires
  dropped_entitlements: string[];
  created_at: Timestamp;
  finished_at: Timestamp | null;
  install_url: string | null;       // for the requesting device, whenever this build is ready
}                                   // (any version, e.g. "install this version")

interface Identity { sha1: string; name: string; team_id: string | null; expires_at: Timestamp | null; }

interface Profile {
  uuid: string; name: string; team_id: string;
  app_id_pattern: string;           // "A1B2C3D4E5.*" or explicit
  wildcard: boolean;
  development: boolean;
  created_at: Timestamp; expires_at: Timestamp;
  devices: number;
  entitlements: string[];           // entitlement keys granted
  usable: boolean;
  reason: string | null;            // why not usable
  uploaded: boolean;                // uploaded to Kiwi (deletable), not found in Xcode's folders
}

interface Signing {
  backend: "keychain" | "files";    // macOS: login keychain + codesign; elsewhere: uploaded .p12 + rcodesign
  identities: Identity[];           // "files": the uploaded identity, if any
  profiles: Profile[];
  active: { identity_sha1: string; profile_uuid: string; expires_at: Timestamp } | null; // default wildcard choice
  error: string | null;
}

interface Settings {
  refresh_interval_minutes: number; // default 60, min 15
  auto_prepare_updates: boolean;    // default true
  notify_updates: boolean;          // default true
  notify_expiry: boolean;           // default true
  devicectl_sync: boolean;          // default true
  devicectl_sync_minutes: number;   // default 10, min 5
  strip_watch_apps: boolean;        // default true
  app_store_country: string;        // default: the Mac's region, else "us"; two letters (stored
                                    // lowercase): the App Store storefront originals are looked up in
  kiwi_builds: boolean;             // default false: Kiwi Builds (see above)
}
```

## Endpoints

| Method | Path | Body | Response |
|---|---|---|---|
| GET | `/api/health` | | `{ ok: true, version, pid }` (`pid`: the server process, for deploy checks) |
| GET | `/api/me` | | `Me` |
| PATCH | `/api/me` | `{ name?, os_version?, platform?: "ios" \| "android" \| null, onboarded?: boolean }` (`os_version: null` clears a manual value; `platform: null` clears the owner's choice, back to detection; `onboarded: true` records the first time the setup was completed, `false` shows it again) | `Me` |
| PUT | `/api/me/packages` | `{ packages: InstalledPackage[] }` (Kiwi's Android app; see "Android devices") | `204`; `400` for an iOS device |
| GET | `/api/sources` | | `Source[]` (by position) |
| POST | `/api/sources` | `{ url }` | `201 Source` (fetched and parsed before saving; `409 conflict` if present). `url` may be an AltStore source, a web page (the AltStore source an `altstore://` or `sidestore://source?url=` button links, else the page's first `.ipa` link) or an IPA, which is downloaded before the answer. `http://` and scheme-less URLs are fetched over HTTPS |
| GET | `/api/icons/:file` | | `image/png`, an icon Kiwi took from an IPA (cacheable) |
| PATCH | `/api/sources/:id` | `{ enabled?, browse?: "all" \| "picked" }` | `Source` (switching to `"picked"` again restarts what counts as new) |
| DELETE | `/api/sources/:id` | | `204` (keeps installs; their apps become orphaned) |
| POST | `/api/sources/:id/refresh` | | `Source` (after the refresh; may download an `ipa` source's new IPA) |
| POST | `/api/sources/refresh` | | `Source[]` (refreshes all enabled) |
| GET | `/api/apps?q=&source=&hidden=` | | `AppSummary[]` (enabled sources, sorted by name; apps hidden from Browse only with `hidden=1`, which also lists the apps made for the other platform) |
| GET | `/api/apps/:source_id/:bundle_id` | | `AppDetail` |
| PATCH | `/api/apps/:source_id/:bundle_id` | `{ hidden?: boolean, original?: { bundle_id: string \| null } \| null }` | `AppDetail`. `hidden` hides or shows it in Browse. `original`: `null` matches by bundle id again, `{ "bundle_id": null }` means not based on an App Store app, otherwise that App Store app (looked up unless cached; `400` when the storefront has no such app) |
| GET | `/api/store/search?term=` | | `StoreApp[]` (iOS apps in the storefront, at most 15; cached so one can be linked) |
| POST | `/api/apps/:source_id/:bundle_id/prepare` | `{ version?, build?, revision? }` (default: target release; build/revision require version) | `202 Build` (existing ready/running build is returned) |
| GET | `/api/builds/:id` | | `Build` |
| GET | `/api/library` | | `AppSummary[]` installed on this device, one per install (`state.installed.signed_bundle_id` is unique; two rows can share `source_id`/`bundle_id`, e.g. Kiwi's build and the App Store build seen by devicectl); orphaned installs have `orphaned: true`, `source_name` `"Removed source"` and `state.action` `"open"` |
| DELETE | `/api/library/:signed_bundle_id` | | `204` (forgets the record; does not uninstall) |
| GET | `/api/updates` | | `AppSummary[]` with action `update` (or `preparing`/`install`/`installing` for an update) |
| POST | `/api/updates/prepare` | | `Build[]` (queues all updates) |
| GET | `/api/signing` | | `Signing` |
| POST | `/api/signing/rescan` | | `Signing` |
| POST | `/api/signing/identity` | `{ p12, password }` (`p12`: the `.p12` file, base64) | `Signing` (after a rescan). `files` backend only; see "Signing uploads" |
| DELETE | `/api/signing/identity` | | `Signing`; `404` when none was uploaded. `files` backend only |
| POST | `/api/signing/profiles` | `{ profile }` (the `.mobileprovision` file, base64) | `Signing` (after a rescan); replaces an upload with the same UUID |
| DELETE | `/api/signing/profiles/:uuid` | | `Signing`; `404` unless the profile was uploaded |
| GET | `/api/settings` | | `Settings` |
| PATCH | `/api/settings` | partial `Settings` | `Settings` |
| POST | `/api/push/subscribe` | `{ endpoint, keys: { p256dh, auth } }` | `204` |
| POST | `/api/push/unsubscribe` | `{ endpoint }` | `204` |
| POST | `/api/push/test` | | `{ sent: number }` |
| GET | `/api/devices` | | `{ devices: Device[], core_devices: CoreDevice[] }` |
| PATCH | `/api/devices/:id` | `{ name?, devicectl_id?: string \| null }` | `Device` |
| POST | `/api/devices/sync` | | `{ devices: Device[], core_devices: CoreDevice[] }` (runs devicectl sync now) |

OTA (no `/api` prefix, no cookies, token-authorized; see architecture.md):

| Method | Path | Response |
|---|---|---|
| GET | `/ota/:token/manifest.plist` | XML plist manifest |
| GET, HEAD | `/ota/:token/app.ipa` | signed IPA, `Range` supported; the install is recorded when a full GET has written the last byte, or when the ranges served for the token cover the whole file |
| GET | `/ota/:token/icon.png` | app icon or 404 |
| GET, HEAD | `/ota/:token/app.apk` | an Android build's APK (`application/vnd.android.package-archive`, `Content-Disposition: attachment; filename="<Name>-<version>.apk"`), `Range` supported; the install is recorded as for `app.ipa` |

Kiwi's Android app (no `/api` prefix, but the owner's devices only, like `/api`):

| Method | Path | Response |
|---|---|---|
| GET, HEAD | `/android/kiwi.apk` | `Me.android_app`'s file (`application/vnd.android.package-archive`, `Content-Disposition: attachment; filename="Kiwi-<version_name>.apk"`), `Range` supported; `404` while Kiwi serves none |

`install_url` = `itms-services://?action=download-manifest&url=` + percent-encoded
`<public_url>/ota/<token>/manifest.plist`. For an Android build it is
`<public_url>/ota/<token>/app.apk`: the browser downloads the file and Android's installer
opens it. A token serves the file of its own kind only: `manifest.plist` and `app.ipa`
answer 404 for an Android build, `app.apk` for an iOS one.

## Device platforms

Every device installs for one platform, `Device.platform`:

1. the one the owner chose (`PATCH /api/me` `platform`, `platform_source: "chosen"`), else
2. the one Tailscale (the node's OS: iOS or Android) or a User-Agent (iOS Safari or another
   iOS browser, the iOS installer, an Android browser, Kiwi's Android app) reported first
   (`"detected"`; `detected_platform` keeps it, also when a choice overrides it). The first
   report wins, so an iPhone never turns into an Android device, and an Android client
   never sets an iPhone's OS version (or the other way round), else
3. iOS (`"assumed"`): a desktop browser tells nothing, so `detected_platform` is null and
   the owner is asked, like on a phone.

The platform decides what Browse lists, which releases are compatible, the warnings, and
what the scheduler prepares and notifies. `Me.features` tells the web app (and Kiwi's
Android app) what to show; `Me.onboarded` whether the first-run setup is done. Devices that
were in use before Kiwi had platforms (with an install, an install attempt or a push
subscription) count as set up.

## Signing uploads

A Kiwi on macOS signs with the login keychain (`Signing.backend: "keychain"`): its identity
comes from the keychain, and `POST`/`DELETE /api/signing/identity` answer `409 conflict`.
Elsewhere (`"files"`) the owner uploads an Apple Development (or Distribution) identity as
a `.p12` with its password ([signing-on-linux.md](signing-on-linux.md) explains how to get
one). Provisioning profiles can be uploaded on every host; they are used together with the
profiles in Xcode's folders.

- The identity is checked before it is stored: it must open with the password, hold a
  code signing certificate with a team id that has not expired, and the private key that
  belongs to it. A `.p12` encrypted with AES (OpenSSL 3's default) is refused with advice
  how to convert it. A new upload replaces the previous identity.
- A profile must parse, be an iOS profile and not have expired.
- Problems answer `400 bad_request` with a message for the owner (`"The password is
  wrong"`, `"'Name' expired on 2026-01-01"`). Bodies are limited to about 100 KB for an
  identity and about 1.4 MB for a profile (`400 "Request body is too large."`).
- Responses never contain the `.p12`, the key or the password; Kiwi stores them in
  `<data>/signing` with mode 0600 and does not log them.

## Android devices

A device is an Android one when its platform is `android` (see "Device platforms"); one
that Tailscale or a User-Agent reported as Android also gets `model: "Android"` while it
has no model. For such a device:

- Releases are compared with platform `android`. Kiwi Builds' Android releases name the
  platform `Android` and declare no `min_os`, so they are offered whatever version the
  browser reports. Releases that name no platform are iOS ones.
- `GET /api/apps` leaves out apps none of whose releases run on the device's platform
  (and for an iPhone, the Android apps). They can still be opened: every release is then
  `compatible: false` with `incompatibility` (and the app's `unavailable`)
  `"Made for iPhone and iPad"` on Android, `"Made for Android devices"` elsewhere.
- An Android app needs no signing. Its build is `ready` from the moment Kiwi Builds
  publishes it, so the app's action is `install` with the APK as `install_url`, and
  `POST .../prepare` answers at once with that `Build` (`expires_at: null`, no
  `dropped_entitlements`). A completed download records the install:
  `Installed.expires_at` is null and `AppState.open_url` is null, since Kiwi cannot open
  or see apps on Android.
- `Me.warnings` omits the signing warnings (`no_identity`, `no_profile`, `expiring`) and
  `unknown_os`. While signing Kiwi's Android app fails (see "Kiwi's Android app"), it has
  `{level: "warning", code: "android_app", link: "/settings"}` with the reason.
- Android apps have no App Store original (`original: null`, `original_match: "none"`).

### Kiwi's Android app

The native app talks to the same API (see "Conventions" for its User-Agent). The phone is
identified through Tailscale like its browser, so Chrome and the app are one device; its
`client` is `android_app`, its `os_version` the Android release (source `android_app`,
above a browser's guess and below a value entered by hand).

`PUT /api/me/packages` reports the packages the app can see (all user-installed ones; the
server ignores the rest, and unknown fields of an entry):

```ts
interface InstalledPackage {
  package: string;                  // "app.morphe.android.youtube"
  version_name: string | null;
  version_code: number;
  signer_sha256: string[];          // SHA-256 (hex) of each signing certificate (DER)
  installer: string | null;         // e.g. "com.android.vending"
}
```

Kiwi reconciles the device's installs with it like a devicectl sync: a package of a Kiwi
Builds Android app signed with this Kiwi's Android key is a verified install (an existing
install stays `origin: "kiwi"`, one Kiwi had no record of becomes `origin: "device"`;
the version is `version_name`); the same package signed with another key is foreign
(`AppState.foreign_install`; Kiwi's download of it did not install, and installing it
fails until the other app is removed); an install whose package is missing is marked
removed (one whose download completed less than 10 minutes ago is kept: Android may
still be installing it). Android's install can fail after the download, and the app
reports right after it: such a report counts for that install once the 10 minutes are
over, without another report (Kiwi keeps each device's last report in memory and checks
every minute). Until the Android runner has recorded Kiwi's key (it does so
whenever it builds or signs Kiwi's app), Kiwi only records removals. Each report replaces
the previous one; at most 5000 packages (2 MiB).

Kiwi offers its Android app (`Me.android_app`) when `<data>/android-app/` holds
`kiwi.apk` and `kiwi.json` (`{package, version_name, version_code, sha256, size}`, other
fields are ignored) describing it. A description that doesn't match the file (size or
SHA-256) is ignored. The web app offers it on Android devices (`android_app_offer`);
`install_url` downloads it from `/android/kiwi.apk`. While Kiwi Builds is on and Kiwi has an
Android device or Android build, Kiwi writes both itself: it signs the unsigned release
pinned in `automation/kiwi-app.json` (or an unsigned APK the owner placed in
`android-app/unsigned/`) with its Android key ([android-app.md](android-app.md)).

Static: every other GET path serves `web/dist`; unknown paths without a file extension
fall back to `index.html`. `/sw.js` and `/index.html` use `Cache-Control: no-cache`;
`/assets/*` use `public, max-age=31536000, immutable`.

## Push payload

```json
{ "title": "YouTubePlus update ready", "body": "21.43.5 → 21.44.0 · Tap to install", "url": "/apps", "badge": 2, "tag": "update:com.google.ios.youtube.A1B2C3D4E5" }
```
