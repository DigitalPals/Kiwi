# Kiwi HTTP API

Contract between `crates/kiwi-server` and `web/`. JSON uses snake_case. Timestamps are
RFC 3339 strings in UTC. All `/api` responses are `application/json` with
`Cache-Control: no-store`.

## Conventions

- Mutating requests (`POST`, `PUT`, `PATCH`, `DELETE`) must send `X-Kiwi: 1`; requests with
  a body must send `Content-Type: application/json` (except a Mac builder's artifact upload,
  see "Mac builder"). Otherwise `403 csrf`. The web app
  sends `{}` for POST/PATCH without parameters and no body for DELETE (it still sends
  both headers); the server accepts empty and `{}` bodies alike.
- Errors: HTTP 4xx/5xx with `{ "error": { "code": string, "message": string } }`.
  `message` is user-readable. Codes used: `bad_request`, `csrf`, `not_found`,
  `conflict`, `source_fetch`, `source_parse`, `signing_unavailable`, `app_store` (502,
  the App Store could not be reached), `device_unidentified` (503), `bad_host` (421),
  `not_owner` (403, the device belongs to someone other than the owner, or the request was
  made on the computer Kiwi runs on, not on a Mac, without Tailscale), `builds_off` (409,
  Kiwi Builds is turned off in Settings), `needs_mac` (409, an iOS app in Kiwi Builds on a
  Kiwi that doesn't run on macOS and has no Mac builder), `builder_unsupported` (409, Mac
  builder routes on a Kiwi that runs on macOS), `builder_unpaired` (401, a Mac builder's
  token is unknown or revoked), `bad_code` (400, a wrong or expired pairing code),
  `rate_limited` (429, too many wrong pairing codes), `task_gone` (410, a Mac builder's task
  Kiwi no longer has), `internal`.
- Every request (`/api`, `/ota` and the web app) must be addressed to the public URL's
  host (`Host`, as `tailscale serve` forwards it) or to the listen address
  (`127.0.0.1:<port>`, `localhost:<port>`, `[::1]:<port>`); an `X-Forwarded-Host`, when
  present, must be the public host. Otherwise `421 bad_host` (blocks DNS rebinding). When
  Kiwi learns its public URL from Tailscale after it started, its host is allowed from then
  on.
- The device is resolved from Tailscale (`X-Forwarded-For` + whois) or the
  `kiwi_device` cookie. `X-Forwarded-For` counts only from `tailscale serve` on loopback:
  on a Mac from any loopback connection, elsewhere only from a connection of tailscaled's
  user (`--proxy-uid`, root by default; architecture.md "Security"). When a request proxied
  from the tailnet cannot be identified because the Tailscale whois fails (retried once),
  the response is `503 device_unidentified` ("Couldn't identify this device through
  Tailscale. Try again."). On a Mac (or with `--trust-local`) the cookie is used without a
  forwarded address or when whois reports that the address is not a peer: a browser on the
  Mac. On other computers
  such a request, and one from the computer's own Tailscale address, could come from
  anyone who can log in there; it gets `403 not_owner` ("Kiwi is used through Tailscale,
  not on the computer it runs on. Open https://kiwi.example.ts.net on a phone or computer
  signed in to Tailscale as alex@example.com.") and makes no device or cookie. The web app
  should send requests with `credentials: "same-origin"`, and on startup completes
  `GET /api/me` (which sets the cookie) before sending other requests, so concurrent first
  requests do not each create a browser device.
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
`android-youtube-music` and `android-reddit`. `:app` below is one of those ids, or the id of
an added [community app](#community-builds) (`community-<package>`); an unknown id returns
404. Responses contain no credentials, signing keys, models, raw Apple or
Google responses, file paths or private logs.

Kiwi Builds is off until the owner turns it on (`Settings.kiwi_builds`). While it is off,
every `/api/automation` endpoint answers `409 builds_off` and nothing is built; turning it
off cancels queued and running builds. Releases already published stay in the Kiwi Builds
source. A Kiwi that had added apps before this setting existed keeps it on.

`GET /api/automation` returns `{apps, checks}`: every app's state (as below, with only its
newest 3 `jobs`) in display order, and the shared Mac setup `checks` (`{name,ready}[]`).

iOS apps are built only on a Mac: the one Kiwi runs on, or a paired [Mac
builder](#mac-builder). On a Kiwi that doesn't run on macOS and has no Mac builder,
`checks` is always `[{"name":"macOS","ready":false}]`, `POST .../add` and `POST .../check`
of an iOS app answer `409 needs_mac` (its message points to Settings › Mac builder), and
added iOS apps are not scheduled. With a Mac builder paired, iOS apps are added, checked and
scheduled as on a Mac, and `checks` starts with `{"name":"Mac builder","ready":…}` (ready
while the builder is online and runs this Kiwi's runner), followed by the checks the Mac's
runner last reported. Android apps build everywhere.

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
  the app can't be built here (an iOS app on a Kiwi that doesn't run on macOS and has no
  Mac builder).
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
build (202). It is idempotent, so a one-tap install can send it again: when the app already
has a queued or running job started with the same preferences, it returns 202 with the
app's state and queues nothing (a scheduled job still waiting then counts as asked for and
moves ahead of the other scheduled jobs); when that job has other preferences it returns `409 conflict` and changes nothing.
`DELETE /api/automation/:app` removes it: its jobs are cancelled and it leaves
the source; installed copies keep working until they expire. Both return the app's state.

`POST /api/automation/:app/check` accepts `{}` or `{"force":true}` and returns 202. It
returns 400 for an app that is not added and 409 when the app already has a queued or
running job. `force` rebuilds an already published recipe and bypasses its three-failure
retry limit. Manual checks work when automatic checks are disabled.
`POST /api/automation/:app/cancel` returns 200: a queued job is cancelled at once; a
running one completes asynchronously and preserves the published release.

One job runs at a time. Jobs someone asked for (`POST .../add` and `POST .../check`) start
before jobs the schedule queued: the worker takes the oldest queued `interactive` job, else
the oldest scheduled one (signing Kiwi's own Android app, which takes seconds, still goes
first). With a Mac builder, iOS jobs don't start while it is offline (stage `Waiting for
<Mac name>`) or runs another runner (`Waiting for an update of Kiwi on <Mac name>`): the
jobs behind them start instead, and they start once the builder can take them. Job statuses are `queued` (waiting for another job; its `stage` names the job that
was running when it was queued), `running`, `ready`, `unchanged`, `blocked`, `failed` or `cancelled`. Each
job has `id`, `app`, `sequence`, `stage`, `created_at`, nullable `finished_at`, `version`,
`external_id`, `recipe_key`, `signed_build_id`, a bounded user-facing `error`, and the
`preferences` it was started with; changing preferences never changes a queued or running
job. `interactive` is `true` on jobs someone asked for and absent on scheduled ones (state
saved by older versions loads as scheduled). A queued job also has `ahead`: how many builds
run before it starts, the running one included (`0` when it starts next and the worker is
free); show it as its place in the queue. `error_kind` says what the owner can do about a
failed or blocked job: `"retry"` (a download or rate limit; try again later),
`"other_bundle"` (the patches failed or don't fit; choose another bundle) or
`"unavailable"` (nothing works with a version Kiwi can download: paid, region-locked or
not on Google Play); it is null otherwise and on Kiwi's own recipes. Poll while
`finished_at` is null. A `ready` job has passed package and signing checks;
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

### Community builds

Android apps patched with bundles from the Morphe community list
([kiwi-builds.md](kiwi-builds.md#community-builds)). Every route needs Kiwi Builds on
(`409 builds_off`). A community app's id is `community-<package>`, where the package matches
`^[a-zA-Z][a-zA-Z0-9_]*(\.[a-zA-Z0-9_]+)+$`. The apps Kiwi Builds builds itself (YouTube,
YouTube Music, Reddit) are not offered as community apps.

`GET /api/community`:

```json
{"available": true, "reason": null,
 "index": {"updated_at": "2026-11-11T06:00:00Z", "apps": 1645, "bundles": 482, "stale": false},
 "popular": [CommunityApp, ...],
 "added": [AutomationState, ...]}
```

- `available` is false, with a one-sentence `reason`, where community apps can't be built:
  not on Linux ("Community builds need Kiwi on Linux or in Docker, where Kiwi can lock the
  patcher in a sandbox."), on a Linux without Landlock and seccomp (the reason names what
  is missing), or before the list was loaded. `popular` is still listed then (browse only).
- `index` is null until the list was first fetched; `popular` is then empty and `reason`
  says Kiwi is loading the community list. `updated_at` is when Kiwi last confirmed its
  copy is current; `stale` is true when the last refresh failed or the copy is older than a
  day (Kiwi keeps using it).
- `popular`: up to 30 apps, the list's featured apps in its order, then the most installed.
- `added`: the community apps the owner installed, in the shape of
  `GET /api/automation/:app` (below).

`GET /api/community/search?q=<text>&offset=0&limit=30` returns `{"results": [CommunityApp,
...], "total": 123}`: apps whose name, package or patch names or descriptions contain `q`
(case-insensitive), apps whose name or package matches first, each group by installs. An
empty `q` lists every app by installs. `limit` is 1 through 100.

CommunityApp:

```json
{"id": "community-com.discord", "package": "com.discord", "name": "Discord",
 "icon_url": "/api/art/…", "category": "Social & Communication", "installs": 700000000,
 "description": "…", "bundles": 4, "patches": 79, "featured": true, "added": false}
```

`icon_url` goes through Kiwi's artwork proxy (or is null); `bundles` is how many GitHub
bundles have patches for the app, `patches` the most any one of them has (list data,
informational).

An added community app's state is the `GET /api/automation/:app` shape with `platform`
`"android"`, `bundle_id` the package the build installs as (the original package until the
first build), `tweak` `"<bundle> <tag>"`, `summary` a short patch summary, `experimental`
true, `warning` a not-reviewed notice, `app_version` the pinned version, `limitations` the
fixed community notices (not reviewed; replaces the Play Store app or installs beside it;
Google sign-in and notifications may not work; Android 12L or later), `included` the
default patches the build applied, `preferences.enabled` false, `next_check_at` null, and:

```json
"community": {"package": "com.discord", "repo": "VenusIsJaded/Venus-Patches",
  "author": "VenusIsJaded", "tag": "v1.21.1", "chosen": "auto",
  "update": null, "resolving": false}
```

- `repo`, `author`, `tag`: the pinned bundle release, null before the first build (`repo`
  then names a bundle the owner chose, if any).
- `chosen`: `"auto"` (Kiwi picked it) or `"manual"` (the owner did).
- `update`: null, or `{"tag":"v1.22.0","published_at":"…"}` once a newer stable release of
  the pinned repository is at least 72 hours old.
- `resolving`: true while a build is choosing its bundle (no tag yet, or a manual choice).

`GET`, `DELETE` and `POST .../cancel` on `/api/automation/:app` accept added community ids
with the same meaning (`DELETE` also forgets the pinned bundle, so the next install picks
again; the publisher's certificate stays pinned). `PATCH` and `POST .../check` and
`.../add` answer 400: community apps have no schedule or options. Community jobs carry
`community`: `{}` for an automatic pick, `{"repo": "owner/name"}` for a manual one or
`{"update": "<tag>"}` for an approved update.

`POST /api/community/:id/install` accepts `{}` or `{"repo": "owner/name"}` (a manual choice,
one of the app's bundles in the list or the pinned one, any letter case) and returns the
app's state. It adds the app and queues an interactive build (202), which starts before
scheduled builds. It joins a queued or running build of the app (202) unless that build
uses another bundle than a requested `repo` (`409 conflict`). When the app's build is
published and the request names no other bundle, it returns 200 without building. Unknown
apps return 404, a malformed or unlisted `repo` 400, and `409 community_unavailable` (with
the reason) where community apps can't be built.

`GET /api/community/:id/candidates` lists the app's bundles from the list and what Kiwi
learned building them, without network access:

```json
[{"repo": "VenusIsJaded/Venus-Patches", "author": "VenusIsJaded", "stars": 180,
  "updated_at": "2026-11-01T00:00:00Z", "patches": 78, "versions": ["351.11 - Stable"],
  "recommended": true, "chosen": true, "failed": null}]
```

In Kiwi's order of preference by the list's data. `patches` and `versions` come from the
bundle's own release once Kiwi checked it (else from the list); `recommended` marks
Kiwi's automatic pick (its pinned automatic choice, else the best bundle not known to
fail); `chosen` the pinned bundle; `failed` why a bundle can't be used at its latest
release, or null.

`POST /api/community/:id/update` approves the offered `update`: the next build pins the
newer release and is queued as an interactive job (202, the app's state). `409` when no
update is offered or another build of the app is on its way.

Stages of community jobs include "Choosing patches", "Checking <owner/repo>",
"Downloading original <App>", "Patching <App>", "Signing", "Validating the patched app"
and "Adding to Kiwi". A job ends at "Available in Kiwi". Published community builds join
the Kiwi Builds source (`kiwi-youtube`) under the package they install as, so the usual
app/prepare/install routes and Kiwi's Android app install them like Kiwi's own builds.

## The owner

Kiwi is one person's: the **owner**, the Tailscale user who owns the Mac (according to
tailscaled), or `--owner`. Kiwi remembers the owner (meta `owner_login`), so they are still
known while Tailscale is down. Devices whose Tailscale user is the owner, browser devices on
the Mac itself and devices seen through Tailscale before Kiwi recorded users are the
owner's. On a computer other than a Mac, other users and services may share it, so a
browser device there is nobody's, and requests made on it without Tailscale get
`403 not_owner` (see "Conventions").

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
    android_signer_sha256: string | null; // SHA-256 (hex) of the certificate of the key Kiwi signs
                                    // Android apps with; null until Kiwi Builds has loaded it
    builds_ios: boolean;            // Kiwi Builds can build iOS apps: on macOS, or with a paired
                                    // Mac builder (also while it is offline)
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
  | "android_app_offer" // the web app on a device detected as Android (not a desktop
                        // browser whose owner chose Android), while android_app is not null
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
// Icons taken from IPAs have `icon_url` `/api/icons/<sha256>.png`; every other icon and
// screenshot URL Kiwi returns (sources, apps, App Store originals) is an `/api/art/...` URL.
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
| GET | `/api/art/:tag?url=` | | A source's or the App Store's artwork, fetched by Kiwi (PNG, JPEG, GIF or WebP; cacheable). Only URLs Kiwi returned work: `tag` is an HMAC of `url` with Kiwi's key (`art.key` in the data directory); `404` otherwise and when the image can't be fetched (not tried again for 15 minutes) |
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
| PATCH | `/api/devices/:id` | `{ name?, devicectl_id?: string \| null }` | `Device`; linking (`devicectl_id` a string) is `400` for a device whose `platform` is not `ios` |
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
answer 404 for an Android build, `app.apk` for an iOS one. A token works for 24 hours,
and while the installer keeps using it (within the last hour) up to 48 hours. Once it is
12 hours old, the next response that includes the build's `install_url` has a new token
(unless the installer used the old one in the last hour; always after 24 hours). A
replaced token that still worked keeps leading to its build for 2 hours (until Kiwi
restarts), so a page or app that still shows it, and a download that started with it, go
on working; an expired one stays expired.

## Mac builder

A Kiwi that doesn't run on macOS builds iOS apps on a paired Mac: the Mac builder (`kiwi
builder run`, installed with `./install.sh --builder`; [kiwi-builds.md](kiwi-builds.md#mac-builder)).
The Mac connects out to Kiwi and asks for work; Kiwi hands it the iOS runner's `plan`,
`build` and `doctor` calls as tasks, never code. One builder per Kiwi: pairing another
replaces it. A Kiwi on macOS builds iOS apps itself: there, every route below answers `409
builder_unsupported` ("This Kiwi runs on a Mac and builds iOS apps itself.") except `GET`,
which reports `supported: false`.

### Settings (owner)

These follow the usual owner and CSRF rules.

`GET /api/builder`:

```json
{"supported": true,
 "reason": null,
 "builder": {
   "name": "Mac Studio", "paired_at": "2026-11-11T09:00:00Z", "last_seen": "2026-11-11T10:02:11Z",
   "online": true, "compatible": true,
   "version": "0.2.0", "os": "macOS 26.1 (arm64)",
   "checks": [{"name": "Mac builder", "ready": true}, {"name": "Mac build tools", "ready": true}],
   "task": {"job": "<job id>", "app": "youtube", "name": "YouTube", "command": "build",
            "stage": "Decrypting YouTube", "started_at": "2026-11-11T10:01:30Z"}
 },
 "pairing": {"code": "K7QF-9XMD-2PLA", "expires_at": "2026-11-11T10:12:00Z",
             "command": "./install.sh --builder --server https://kiwi.example.ts.net --pair K7QF-9XMD-2PLA"}}
```

- `builder` is null while none is paired. `online`: it called within the last 60 s.
  `compatible`: its runner.py and recipes.json are this Kiwi's (same SHA-256); otherwise iOS
  jobs wait and Settings should say "Kiwi on <name> is a different version. On the Mac: `git
  pull && ./install.sh --builder`." `last_seen` may be null after a restart until it calls.
  `checks` is `GET /api/automation`'s. `task` is the build job it works on (`app` is the
  recipe id, `command` `plan` or `build`, `stage` the job's), or null; health checks are not
  shown.
- `pairing` is the unused code, or null. `command` uses Kiwi's public URL.

| Method | Path | Response |
|---|---|---|
| POST | `/api/builder/pairing` | `201 {code, expires_at, command}`: a one-time code of 12 characters from `23456789ABCDEFGHJKLMNPQRSTUVWXYZ` (60 bits), shown as `XXXX-XXXX-XXXX`, valid 10 minutes; replaces an unused code |
| DELETE | `/api/builder/pairing` | `204`: withdraws the code |
| DELETE | `/api/builder` | `204`: unpairs (revokes the token, ends its task as a failed, retryable job and cancels queued iOS jobs); `404` when none is paired |
| POST | `/api/builder/check` | `202`: the builder runs `doctor` at its next poll (while Kiwi Builds is on); `409 conflict` when none is paired |

### The builder's routes

The builder sends `X-Kiwi: 1` and, except to pair, `Authorization: Bearer <token>`; an
unknown or revoked token is `401 builder_unpaired`. Each request must also pass Kiwi's
identity rule: through `tailscale serve` from a device of the owner's Tailscale user (not
this computer's own node), or on the computer itself where it counts as the owner's (a Mac,
or `--trust-local`; this covers an SSH tunnel to the loopback port); otherwise `403
not_owner`. The builder is not a device: it never appears in `/api/devices`. These routes
work while Kiwi Builds is off (there is no work then).

- `POST /api/builder/pair` `{"code","name","version","os","runner_sha256","recipes_sha256"}`
  (the code in any case, with or without dashes; `name` up to 64 characters) → `201
  {"token","kiwi","name"}`. `token` is 32 random bytes, base64url (43 characters); Kiwi keeps
  only its SHA-256. A wrong or expired code is `400 bad_code`; after 5 wrong codes in 10
  minutes every attempt is `429 rate_limited` until the window passes.
- `POST /api/builder/poll` `{"version","os","runner_sha256","recipes_sha256","name"}` →
  waits up to 25 s, then `200 {"task": Task}` or `204`. Records `last_seen` and what the
  builder reports. A poll means the builder runs no task: a task it had taken fails as lost.
  `Task = {"id": "<24 hex>", "command": "plan"|"build"|"doctor", "recipe": "youtube"|null,
  "request": {...}, "deadline": "<RFC 3339>"}`. For `plan` and `build`, `id` is the job's id
  (the runner's `--job-id`) and `request` is the runner's request file. A `doctor` task (no
  recipe, `request` `{}`) comes when the builder connects, every 6 hours and after `POST
  /api/builder/check`, while Kiwi Builds is on and the builder is compatible.
- `POST /api/builder/tasks/:id/events` `{"events": [<runner events, verbatim>]}` (at most 3
  MiB per request) → `200 {"cancel": bool}`. An empty list is a heartbeat; send one at least
  every 30 s while a task runs. Kiwi checks the events as it checks a local runner's: known
  event names only, `stage` at most 256 bytes, `health` updates `checks`, at most 2 MiB of
  events per task. An invalid event fails the job and answers `cancel: true`, as does a job
  that was cancelled (by the owner, by turning Kiwi Builds off or removing the app).
- `PUT /api/builder/tasks/:id/artifact?sha256=<64 hex>`: the body is the IPA
  (`Content-Type: application/octet-stream`, `Content-Length` required, at most 4 GiB). Kiwi
  streams it into `automation/`, hashes it and moves it to `automation/artifacts/<sha256>.ipa`
  → `204`; `400` when it doesn't match the digest (nothing is kept). Only after the `artifact`
  event with that digest, in a `build` task (else `409`).
- `POST /api/builder/tasks/:id/finish` `{"status": "ok"|"failed"|"cancelled", "error":
  null|string, "provenance": null|object}` → `204`. `ok` for a build needs the upload first
  (`409`); `provenance` (its `sha256` must be the upload's, else `400`) is written to
  `automation/artifacts/<sha256>.json`. The job then goes on as after a local run (an
  `error` event's message wins over `error`; `cancelled` the builder chose itself fails the
  job as retryable).

A task is gone (`410 task_gone` for its events, upload and finish) once it finished, after
Kiwi restarted, when its job was cancelled (events answer `cancel: true` first), and when
the builder made no call for it for 120 s: Kiwi then fails the job, retryable ("Lost
contact with <name> during the build."), without counting it against the recipe. Kiwi
enforces the usual deadlines (plan 10 minutes, build 2 hours).

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

`PUT /api/me/packages` reports the packages Kiwi cares about that are installed (the app's
own, the Android apps of `GET /api/apps?hidden=1` and the device's installs from
`GET /api/library`; the server ignores other packages, and unknown fields of an entry):

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
still be installing it). When the app downloaded the APK itself, it installs it at once
and reports right after, and Android's install can fail after the download: such a report
counts for that install once the 10 minutes are over, without another report (Kiwi keeps
the device's last report in memory while such an install waits for it, and checks every
minute). A download in the browser waits until the owner opens it in Android's installer,
so for it only reports made after the 10 minutes count. Until the Android runner has
recorded Kiwi's key (it does so whenever it builds or signs Kiwi's app), Kiwi only records
removals. Each report replaces the previous one; at most 5000 packages (2 MiB).

Kiwi offers its Android app (`Me.android_app`) when `<data>/android-app/` holds
`kiwi.apk` and `kiwi.json` (`{package, version_name, version_code, sha256, size}`, other
fields are ignored) describing it. A description that doesn't match the file (size or
SHA-256) is ignored. The web app offers it on devices detected as Android
(`android_app_offer`; a desktop browser can't install it, even when its owner chose Android);
`install_url` downloads it from `/android/kiwi.apk`. While Kiwi Builds is on and the owner has
an Android device or Android build, Kiwi writes both itself: it signs the unsigned release
pinned in `automation/kiwi-app.json` (or an unsigned APK the owner placed in
`android-app/unsigned/`) with its Android key ([android-app.md](android-app.md)).

Static: every other GET path serves `web/dist`; unknown paths without a file extension
fall back to `index.html`. `/sw.js` and `/index.html` use `Cache-Control: no-cache`;
`/assets/*` use `public, max-age=31536000, immutable`.

## Push payload

```json
{ "title": "YouTubePlus update ready", "body": "21.43.5 → 21.44.0 · Tap to install", "url": "/apps", "badge": 2, "tag": "update:com.google.ios.youtube.A1B2C3D4E5" }
```
