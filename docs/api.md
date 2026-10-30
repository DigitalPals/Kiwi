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
  `forbidden` (403, only the owner may do this), `not_registered` (403, this iPhone is not
  registered for the family yet), `not_in_profile` (409, the profile Kiwi signs with does not
  include this iPhone yet), `apple` (502, App Store Connect refused or could not be reached),
  `builds_off` (409, Kiwi Builds is turned off in Settings), `internal`.
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
- Roles (see "Family" below): the **owner** (the Tailscale user who owns the Mac, or
  `--owner`) and browser devices on the Mac itself may use every endpoint. **Members** (other
  Tailscale users) may use `/api/health`, `/api/me` (and `PATCH`, `POST /api/me/enroll`),
  `/api/apps` (`GET` and `prepare`), `/api/builds/:id`, `/api/library`, `/api/updates`
  (and `prepare`) and `/api/push/*`; everything else answers `403 forbidden`. A member only
  sees the apps the owner allowed for them, and only once their iPhone is ready
  (`Me.registration.ready`); until then `/api/apps` is empty and `prepare` answers
  `403 not_registered`. Apps they may not use answer `404`.

## Kiwi Builds (automation)

Kiwi patches and signs App Store apps on the Mac from the recipes in
`automation/recipes.json`: `youtube`, `youtube-music`, `twitch`, `x` and `reddit`. `:app` below is one
of those ids; an unknown id returns 404. Responses contain no credentials, models, raw
Apple responses, file paths or private logs.

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
- `checks`: the shared Mac setup checks.

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

Published builds use source `kiwi-youtube` (its id predates the other apps) with each
app's original bundle id, and the usual app/prepare/install APIs. Its source URL is an
internal identifier; refreshing it returns the saved local catalogue without fetching the
network. Raw artifacts and provenance files have no HTTP download route. See
[kiwi-builds.md](kiwi-builds.md) for Mac setup and operational details.

## Family

Kiwi serves the owner's family. A **person** is a Tailscale user (`id` is their Tailscale
login, e.g. `noah@example.com`; nodes without a user get `node:<StableID>`). The **owner** is the
Tailscale user who owns the Mac (according to tailscaled), or `--owner`; legacy devices seen before
people existed and browser devices on the Mac count as the owner's. Everyone else is a
**member**: they see only the apps the owner allowed for them, once their iPhone is ready.

An iPhone becomes ready in three steps:

1. **Enrollment.** `POST /api/me/enroll` returns `{ url }`; the web app opens it in Safari, which
   downloads a one-time "Profile Service" configuration profile. When the user installs it in
   Settings, iOS posts the UDID, model (`PRODUCT`), OS build (`VERSION`) and device name to
   `POST /enroll/:token` (CMS-signed plist, not verified; the 128-bit token, valid for an
   hour, authorizes it), and Kiwi answers `301` to `<public_url>/?enrolled=1`. Nothing stays
   installed. The owner's own devices are approved at once.
2. **Approval.** A member's device is `pending` until the owner approves or declines it; the
   owner's devices get a push notification ("Register Noah's iPhone?", url
   `/settings/family/devices/<id>`).
3. **Registration with Apple.** With an App Store Connect API key (`PUT /api/family/apple`),
   Kiwi registers approved UDIDs in the developer account and creates its own development
   profiles ("Kiwi …") for the team wildcard App ID, and for explicit App IDs of apps it
   signs, that include every enabled device; it saves them in `<data>/profiles` and rescans.
   Without a key, the owner adds the UDID in the Apple Developer portal and a profile that
   includes it (Xcode's or a manual one); Kiwi notices it on the next signing scan. The
   device is **ready** when it is approved and the wildcard profile Kiwi signs with includes
   its UDID; Kiwi then notifies it ("This iPhone is ready").

Profiles that include more of the approved devices are preferred when signing, then the usual
order (explicit before wildcard, latest expiry). A ready build is only installed on a device
whose UDID its profiles include; otherwise `prepare` builds again, or answers
`409 not_in_profile` when no profile includes the device yet.

Which apps a member may use: an explicit choice per app (`PATCH .../apps`), else, while
`new_apps` is on, every app Kiwi first saw after `new_apps` was turned on ("Apps You Add
Later"). Turning an app off hides it and stops its updates; an installed copy keeps working
until it expires. The owner's Browse choices (hidden/picked apps) do not affect members.

```ts
type RegistrationStatus = "none" | "pending" | "approved" | "declined";

interface Registration {
  status: RegistrationStatus;
  udid: string | null;
  product: string | null;           // "iPhone16,2" (from enrollment, else the installer UA)
  os_build: string | null;          // "24A434"
  requested_at: Timestamp | null;
  decided_at: Timestamp | null;
  apple_registered: boolean;        // in the developer account (registered by Kiwi or in a profile)
  in_profile: boolean;              // the wildcard profile Kiwi signs with includes the UDID
  ready: boolean;                   // the device may install apps (always true for the owner)
  error: string | null;             // last registration error (owner views only)
}

interface FamilyDevice {
  id: string;
  name: string;
  person_id: string | null;
  model: string | null;             // machine identifier, e.g. "iPhone16,2"
  os_version: string | null;
  last_seen: Timestamp;
  registration: Registration;
}

interface Person {
  id: string;                       // Tailscale login, or "node:<StableID>"
  name: string;                     // Tailscale display name unless renamed
  role: "owner" | "member";
  new_apps: boolean;                // "Apps You Add Later" are allowed
  app_count: number;                // apps they may use (every app for the owner)
  devices: FamilyDevice[];
  last_seen: Timestamp;
}

interface PersonApp {               // one catalog app, for a person
  source_id: string;
  source_name: string;
  bundle_id: string;
  name: string;
  subtitle: string | null;
  icon_url: string | null;
  tint_color: string | null;
  allowed: boolean;
  explicit: boolean;                // chosen by hand (else it follows `new_apps`)
  installed_version: string | null; // on one of the person's devices
}

interface PersonDetail extends Person {
  apps: PersonApp[];                // every app of the enabled sources, by name
}

interface AppleAccount {
  connected: boolean;               // an App Store Connect API key is saved
  issuer_id: string | null;
  key_id: string | null;
  devices: number | null;           // devices in the developer account at the last check
  device_limit: number;             // 100 per device class and membership year
  checked_at: Timestamp | null;
  error: string | null;             // last App Store Connect error
  profile: {                        // the wildcard profile Kiwi signs with
    uuid: string; name: string; devices: number; expires_at: Timestamp; kiwi: boolean;
  } | null;
}

interface Family {
  people: Person[];                 // the owner first, then by name
  pending: number;                  // devices waiting for approval
  apple: AppleAccount;
}
```

| Method | Path | Body | Response |
|---|---|---|---|
| POST | `/api/me/enroll` | | `{ url }` (a one-time enrollment profile URL for this device) |
| GET | `/api/family` | | `Family` |
| GET | `/api/family/people/:id` | | `PersonDetail` |
| PATCH | `/api/family/people/:id` | `{ name?, new_apps? }` | `PersonDetail` |
| PATCH | `/api/family/people/:id/apps` | `{ apps: { source_id, bundle_id, allowed }[] }` | `PersonDetail` (not for the owner: `400`) |
| DELETE | `/api/family/people/:id` | | `204`: forgets a member, their choices and their devices' registrations (installed apps keep working until they expire; the devices stay in the Apple account) |
| GET | `/api/family/devices/:id` | | `FamilyDevice` |
| POST | `/api/family/devices/:id/approve` | | `FamilyDevice` (starts registration with Apple when a key is saved) |
| POST | `/api/family/devices/:id/decline` | | `FamilyDevice` |
| POST | `/api/family/devices/:id/register` | | `202 FamilyDevice` (tries the registration with Apple again) |
| DELETE | `/api/family/devices/:id/registration` | | `204` (the device must enroll again) |
| GET | `/api/family/apple` | | `AppleAccount` |
| PUT | `/api/family/apple` | `{ issuer_id, key_id, private_key }` (`.p8` text) | `AppleAccount` (checked with App Store Connect first: `400`/`502 apple` with its message) |
| DELETE | `/api/family/apple` | | `AppleAccount` (forgets the key) |
| POST | `/api/family/apple/sync` | | `AppleAccount` (registers approved devices and refreshes profiles now) |

Enrollment (no `/api` prefix, token-authorized):

| Method | Path | Response |
|---|---|---|
| GET | `/enroll/:token/kiwi.mobileconfig` | `application/x-apple-aspen-config` Profile Service payload |
| POST | `/enroll/:token` | `301` to `<public_url>/?enrolled=1` (`400` for an unknown or expired token or a body without a UDID) |

Push payloads: to the owner's devices `{ "title": "Register Noah's iPhone?", "body": "Noah ·
iPhone 15. Tap to choose apps and approve.", "url": "/settings/family/devices/<id>", "tag":
"register:<id>" }`; to a member's device once it is ready `{ "title": "Kiwi", "body": "This
iPhone is ready. Open Kiwi to install apps.", "url": "/", "tag": "registered" }`.

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
  warnings: Warning[];              // members only get warnings about their own installs and iOS version
  access: Access;
  registration: Registration;       // this device's family registration
}

interface Access {
  role: "owner" | "member";
  person: { id: string; name: string } | null;  // null for browser devices on the Mac
  owner_name: string;               // the owner's first name ("Alex"), for "Apps from Alex"
}

interface Source {
  id: string;
  kind: "feed" | "local_build";     // local_build is Kiwi's generated source
  url: string;
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
  orphaned: boolean;                // installed from a source that was removed, or an app the
                                    // member may no longer use (detail URL is 404)
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
  access: AppAccess[] | null;       // owner only: who may install it (owner first); null for members
}

interface AppAccess {
  person_id: string;
  name: string;
  role: "owner" | "member";         // the owner always has every app
  allowed: boolean;
  installed_version: string | null; // installed on one of the person's devices
  pending: boolean;                 // the person has no ready device yet
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
| POST | `/api/sources` | `{ url }` | `201 Source` (fetched and parsed before saving; `409 conflict` if present) |
| PATCH | `/api/sources/:id` | `{ enabled?, browse?: "all" \| "picked" }` | `Source` (switching to `"picked"` again restarts what counts as new) |
| DELETE | `/api/sources/:id` | | `204` (keeps installs; their apps become orphaned) |
| POST | `/api/sources/:id/refresh` | | `Source` (after the refresh) |
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
