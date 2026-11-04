# Kiwi HTTP API

Contract between `crates/kiwi-server` and `web/`. JSON uses snake_case. Timestamps are
RFC 3339 strings in UTC. All `/api` responses are `application/json` with
`Cache-Control: no-store`.

## Conventions

- Mutating requests (`POST`, `PATCH`, `DELETE`) must send `X-Kiwi: 1`; requests with a
  body must send `Content-Type: application/json`. Otherwise `403 csrf`. The web app
  sends `{}` for POST/PATCH without parameters and no body for DELETE (it still sends
  both headers); the server accepts empty and `{}` bodies alike.
- Errors: HTTP 4xx/5xx with `{ "error": { "code": string, "message": string } }`.
  `message` is user-readable. Codes used: `bad_request`, `csrf`, `not_found`,
  `conflict`, `source_fetch`, `source_parse`, `signing_unavailable`, `app_store` (502,
  the App Store could not be reached), `device_unidentified` (503), `bad_host` (421),
  `not_owner` (403, the device belongs to someone other than the owner), `builds_off` (409,
  Kiwi Builds is turned off in Settings), `internal`.
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

## Kiwi Builds (automation)

Kiwi builds and signs apps on the Mac from the recipes in
`automation/recipes.json`: `youtube`, `youtube-music`, `twitch`, `x`, `reddit`, `rambler` and `gboard`. `:app` below is one
of those ids; an unknown id returns 404. Responses contain no credentials, models, raw
Apple responses, file paths or private logs.

`rambler` is an experimental native Swift app, version 0.1.0, compiled from sources
embedded in Kiwi. It uses full Xcode without App Store enrollment or tweak tools. Its
build verifies a synthetic speech correction before the usual signing and publication.
It has no included or optional patches. Other recipes patch App Store apps.

`gboard` is an experimental integration into Gboard 2.3.19. It embeds the native
Rambler client, preserves and validates the keyboard extension, and requires both
bundles' signing profiles to grant the recipe's shared App Groups. It has no optional
patches. Its plan includes the native source digest and Xcode toolchain in addition
to the Apple build id. A successful publication does not imply an actual keyboard
runtime test on an iPhone.

Kiwi Builds is off until the owner turns it on (`Settings.kiwi_builds`). While it is off,
every `/api/automation` endpoint answers `409 builds_off` and nothing is built; turning it
off cancels queued and running builds. Releases already published stay in the Kiwi Builds
source. A Kiwi that had added apps before this setting existed keeps it on.

`GET /api/automation` returns `{apps, checks}`: every app's state (as below, with only its
newest 3 `jobs`) in display order, and the shared Mac setup `checks` (`{name,ready}[]`).

`GET /api/automation/:app` returns one app's state with up to 30 `jobs`, newest first:

- From the recipe: `id`, `name`, `bundle_id`, `icon_url` (App Store artwork or null),
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
- `next_check_at`: scheduled UTC time, or null when not added, disabled or a job is active.
- `checks`: the app's most recently reported requirements. Native apps report their
  Xcode check separately and do not overwrite the overview's App Store setup checks.

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
it does not imply a test on a physical device.
`external_id` is Apple's build identifier for App Store recipes, and
`native-<source digest prefix>` for native apps. Native build identity also includes
the bundled sources, recipe, dependency pins, runner and Xcode toolchain.

Published builds use source `kiwi-youtube` (its id predates the other apps) with each
app's recipe bundle id, and the usual app/prepare/install APIs. Its source URL is an
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

interface Device {
  id: string;                       // "ts:<StableID>" | "local:<random>"
  name: string;                     // user-editable; defaults to Tailscale name
  kind: "tailscale" | "browser";
  tailscale_name: string | null;    // "iphone194"
  os_version: string | null;        // "27.0.1"
  os_version_source: "devicectl" | "installer" | "manual" | "safari" | null;
  model: string | null;             // "iPhone19,4"
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
  code: string;                     // e.g. "no_identity", "no_profile", "expiring", "source_errors"
  message: string;
  link: string | null;              // in-app route to fix it, e.g. "/settings", "/sources"
}

interface Me {
  device: Device;
  server: {
    version: string;
    public_url: string;             // "https://mac-studio.example.ts.net"
    team_id: string | null;
    devicectl_available: boolean;
  };
  push: { vapid_public_key: string; subscribed: boolean };  // subscribed: this device has >= 1 subscription
  counts: { updates: number; installed: number; sources: number; source_errors: number; preparing: number };
  warnings: Warning[];
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
  install_url: string | null;       // itms-services URL when a ready build exists for target
  open_url: string | null;          // "kiwi-xxxxxxxxxxxx://" when installed with Kiwi's scheme
  replaces: Replaces | null;        // installing this app replaces another source's app
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
  signed_bundle_id: string;         // what Kiwi will install it as
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
  size: number | null;              // signed IPA bytes
  expires_at: Timestamp | null;
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
}

interface Signing {
  identities: Identity[];
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
| PATCH | `/api/me` | `{ name?, os_version? }` (`os_version: null` clears a manual value) | `Me` |
| GET | `/api/sources` | | `Source[]` (by position) |
| POST | `/api/sources` | `{ url }` | `201 Source` (fetched and parsed before saving; `409 conflict` if present). `url` may be an AltStore source, a web page (the AltStore source an `altstore://` or `sidestore://source?url=` button links, else the page's first `.ipa` link) or an IPA, which is downloaded before the answer. `http://` and scheme-less URLs are fetched over HTTPS |
| GET | `/api/icons/:file` | | `image/png`, an icon Kiwi took from an IPA (cacheable) |
| PATCH | `/api/sources/:id` | `{ enabled?, browse?: "all" \| "picked" }` | `Source` (switching to `"picked"` again restarts what counts as new) |
| DELETE | `/api/sources/:id` | | `204` (keeps installs; their apps become orphaned) |
| POST | `/api/sources/:id/refresh` | | `Source` (after the refresh; may download an `ipa` source's new IPA) |
| POST | `/api/sources/refresh` | | `Source[]` (refreshes all enabled) |
| GET | `/api/apps?q=&source=&hidden=` | | `AppSummary[]` (enabled sources, sorted by name; apps hidden from Browse only with `hidden=1`) |
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

`install_url` = `itms-services://?action=download-manifest&url=` + percent-encoded
`<public_url>/ota/<token>/manifest.plist`.

Static: every other GET path serves `web/dist`; unknown paths without a file extension
fall back to `index.html`. `/sw.js` and `/index.html` use `Cache-Control: no-cache`;
`/assets/*` use `public, max-age=31536000, immutable`.

## Push payload

```json
{ "title": "YouTubePlus update ready", "body": "21.43.5 → 21.44.0 · Tap to install", "url": "/apps", "badge": 2, "tag": "update:com.google.ios.youtube.A1B2C3D4E5" }
```
