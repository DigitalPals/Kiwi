# Kiwi architecture

Kiwi is a private, self-hosted app store for sideloading IPAs onto your own iPhones. It runs
on an always-on Mac, is reachable only over Tailscale, and needs no
Apple ID sign-in, anisette server or pairing file in the install path:

1. Kiwi reads AltStore-format sources (for example
   `https://raw.githubusercontent.com/<owner>/<repo>/main/apps.json`).
2. It downloads a release IPA and re-signs it on the Mac with Apple's `codesign`, using the
   Apple Development identity in your login keychain and the development provisioning
   profiles Xcode manages (`~/Library/Developer/Xcode/UserData/Provisioning Profiles/`, also
   `~/Library/MobileDevice/Provisioning Profiles/`).
3. The phone installs it over the air with
   `itms-services://?action=download-manifest&url=<https manifest>`. HTTPS with a trusted
   certificate comes from `tailscale serve` (`https://mac-studio.example.ts.net`).
4. The phone uses Kiwi as a Home Screen web app (PWA): browse, install, open, update, badge
   and web push notifications.

Optionally, Kiwi Builds patches App Store apps with open-source tweaks on the same Mac and
publishes them as a built-in source ([kiwi-builds.md](kiwi-builds.md)). It is off until the
owner turns it on in Settings. It can also patch Android apps, which Kiwi serves to the
owner's Android devices as APK downloads (see "Android devices").

```
iPhone (Safari / Home Screen app)
   │  HTTPS, tailnet only
   ▼
tailscale serve  https://<mac>.<tailnet>.ts.net/  ──►  kiwi on 127.0.0.1:8742
                                                         ├─ SQLite + files in ~/Library/Application Support/Kiwi
                                                         ├─ codesign / security (login keychain)
                                                         ├─ tailscaled LocalAPI (who is asking)
                                                         ├─ devicectl (paired iPhones, optional)
                                                         └─ Web Push (Apple's push service)
```

Kiwi runs as the LaunchAgent `local.kiwi` in the user's GUI (Aqua) session: `codesign` fails
with `errSecInternalComponent` from a plain SSH session because the login keychain is not
usable there, and works in the GUI session, so its child processes can sign directly.

## Repository layout

```
Cargo.toml              workspace (shared dependency versions in [workspace.dependencies])
crates/kiwi-source      AltStore source fetching/parsing, release selection, IPA download+validation
crates/kiwi-signer      provisioning profiles, identities, IPA re-signing with codesign (macOS)
crates/kiwi-device      devicectl wrapper, Tailscale (LocalAPI or CLI), user-agent parsing
crates/kiwi-push        VAPID keys + Web Push (RFC 8030/8291/8292) sender
crates/kiwi-server      axum HTTP server, SQLite state, jobs, scheduler; binary `kiwi`
web/                    Vite + Preact + TypeScript PWA (built to web/dist, served by kiwi-server)
automation/             Kiwi Builds: the build runner (Python) and pinned app recipes
install.sh              installs or updates Kiwi on the Mac (build, LaunchAgent, tailscale serve)
uninstall.sh            removes it again
deploy/                 LaunchAgent and log rotation templates; deploy.sh deploys a checkout
                        from another machine over SSH
docs/                   this file, the HTTP API (api.md) and the guides
```

Library crates do not depend on each other; only `kiwi-server` depends on them. All crates
compile and their platform-neutral tests pass on Linux; macOS-only behaviour (codesign,
devicectl) is exercised on a Mac.

## Crates

### kiwi-source

AltStore source handling: canonical and GitHub URL handling (raw links and `blob` links),
URL validation (HTTPS, no credentials, no private or loopback targets), size limits,
conditional requests (ETag/Last-Modified), JSON5-tolerant parsing of the legacy and current
AltStore formats, release ordering, `release_change`, IPA download with SHA-256 and size
checks, progress and cancellation, and safe archive inspection (`inspect_ipa`,
`validate_metadata`). It also parses `tintColor`, `category` and permissions
(`appPermissions` with entitlements and privacy descriptions, and the legacy
`permissions`). Sources stay `Serialize + Deserialize`; Kiwi persists them in SQLite. The
OS version of a device is optional: without it every release is eligible.

`SourceClient::find` tells what a pasted URL offers: an AltStore source, a download page
(scanned for `altstore://`/`sidestore://source?url=` buttons, `.ipa` links and its
`apple-touch-icon`; a source wins over an IPA) or an IPA. Apps added by link
(`Source::link`) are downloaded with `download_linked` (inspected, not validated against a
release), and `ipa_icon` converts their loose Xcode icon (CgBI PNG) into a standard PNG. The
server's `linked` module builds their one-app catalog and refreshes it: page first, then a
`HEAD` request whose ETag, Last-Modified and length are compared with the stored ones.

### kiwi-signer (macOS)

Lists signing identities (`security find-identity` / `find-certificate`), loads and parses
provisioning profiles in pure Rust (the plist embedded in the CMS envelope: team, App ID
pattern, devices, entitlements, certificates, expiry, `get-task-allow`), matches a bundle id
against a profile (exact before wildcard), checks a profile is usable with an identity
(certificate in profile, team matches, not expired) and re-signs IPAs. A signed bundle id is
`<original>.<team>` (`com.google.ios.youtube.A1B2C3D4E5`); its URL scheme is `kiwi-` plus 12
hex characters of its SHA-256.

Signing algorithm:

1. Validate the archive (no absolute paths, no `..`, no symlinks escaping the root,
   exactly one `Payload/*.app`) then extract preserving symlinks and permissions
   (with `/usr/bin/ditto -x -k`, after validation).
2. Remove `iTunesMetadata.plist`, `META-INF`, `SC_Info`, `UISupportedDevices`; remove
   `Watch/` when `strip_watch`.
3. Set main `CFBundleIdentifier` to `bundle_id`; for every nested bundle (`PlugIns/*.appex`,
   `Extensions/*.appex`, nested `*.app`) map `OLD.suffix` to `NEW.suffix`, otherwise
   `NEW.<last component>`; also rewrite `WKCompanionAppBundleIdentifier` /
   `NSExtension.NSExtensionAttributes.WKAppBundleIdentifier` when present.
4. Inject `CFBundleURLTypes` entry `{CFBundleURLName: "kiwi", CFBundleURLSchemes: [url_scheme]}`
   into the main app's Info.plist (keep existing entries).
5. For each app/appex choose a profile: exact App ID match first, then wildcard; it must
   be usable with the identity (certificate in profile, team matches, not expired).
   Build entitlements from that profile: `application-identifier = TEAM.<id>`,
   `com.apple.developer.team-identifier`, `get-task-allow` from the profile,
   `keychain-access-groups = [TEAM.<id>]`, plus every other entitlement the profile
   grants that the original binary requested (for explicit App IDs, e.g. app groups).
   Record original entitlements (`codesign -d --entitlements - --xml`, empty when
   unsigned) and which were dropped.
6. Sign inside-out (deepest path first): dylibs and frameworks plain, appex/nested apps
   with `embedded.mobileprovision` + entitlements, then the main app:
   `codesign --force --timestamp=none --generate-entitlement-der -s <sha1> [--entitlements f] <path>`.
7. `codesign --verify --deep --strict` on the main app, then zip `Payload` into
   `output_ipa` (`/usr/bin/zip -qry`, symlinks preserved).

Signing reports its steps (extracting, preparing, signing n of m, verifying, packaging), can be
cancelled, and returns the new bundle ids, the profiles used, the expiry, and the original and
dropped entitlements (shown on an app's page before it is installed).

### kiwi-device

`xcrun devicectl` (list paired devices and their apps, install), Tailscale identity and
user-agent parsing. devicectl JSON (`jsonVersion` 5) prefers the new `properties` dictionary
and falls back to the deprecated `hardwareProperties`/`deviceProperties`/
`connectionProperties`. Every command runs with a timeout, `--quiet`, and a temporary
`--json-output` file. Without a paired device (or Xcode) the features that need it degrade
gracefully.

Tailscale is asked through tailscaled's LocalAPI socket (one HTTP request, 10 s timeout):
`whois?addr=<ip>` for the node and user behind a request (`404` for a non-peer) and
`status?peers=false` for this Mac's own user and MagicDNS name. Only where the socket does not
exist (the Mac App Store and standalone Tailscale apps have none) it runs the `tailscale` CLI
(`whois --json`, `status --json`); the socket is looked for on every call. A socket that exists
but does not answer is an error, not a reason to try the CLI, which would use the same socket.

User agents: Safari `Version/27.0` gives an approximate OS version (the `iPhone OS 18_6` token is
frozen and is ignored); the OTA installer (`... iOS/27.0.1 model/iPhone19,4 build/24A434 ...`)
gives the exact OS version and model.

### kiwi-push

VAPID keys (P-256, created with mode 0600) and a Web Push sender: RFC 8291 `aes128gcm`
content encoding, RFC 8292 VAPID (ES256 JWT, `aud` = endpoint origin, `exp` <= 12 h, `sub` =
the public URL or a `mailto:`), TTL, urgency and topic. Outcomes are delivered, gone
(404/410: delete the subscription) or rejected. Only HTTPS endpoints of known push services
are allowed (`*.push.apple.com`, `fcm.googleapis.com`, `*.mozilla.com`,
`*.notify.windows.com`). Tested against the RFC 8291 Appendix A vector.

## Server (kiwi-server, binary `kiwi`)

- axum on `127.0.0.1:8742` (`--listen`). Only `tailscale serve` exposes it.
- Flags (each also an environment variable, `KIWI_<FLAG>`): `--data-dir` (default
  `~/Library/Application Support/Kiwi`), `--web-dir` (default `<exe dir>/web`),
  `--public-url` (default `https://<this Mac's MagicDNS name>`, asked from Tailscale at start;
  without Tailscale, the listen address with a warning; also the only host name requests may
  use besides the listen address, see "Security"), `--tailscale-socket` (tailscaled's
  LocalAPI, default `/var/run/tailscaled.socket`), `--tailscale-bin` (the CLI, used without
  that socket; default `/opt/homebrew/bin/tailscale`), `--owner` (the owner's Tailscale
  login, see "The owner"), `--allow-simulators` (treat simulators as linkable devices, for
  testing only). `install.sh` passes the address and the Tailscale CLI it found explicitly.
- SQLite (`rusqlite`, bundled) at `<data>/kiwi.db`, migrations embedded and versioned.
- Data layout: `downloads/<sha256>.ipa` (content addressed), `builds/<build_id>/<Name>.ipa`,
  `work/` (temporary, cleared at start), `icons/<sha256(url)>.png`, `vapid.json` (0600),
  `automation/` (Kiwi Builds), `kiwi.lock`.
- At start Kiwi takes an exclusive lock on `<data>/kiwi.lock` before any startup cleanup
  (clearing `work/`, failing interrupted builds); a second instance exits with an error
  instead of damaging the running one.
- Logging with `tracing` to stdout/stderr (LaunchAgent redirects to
  `~/Library/Logs/Kiwi/kiwi.log`).

### State

- `sources`: id (`source_key(url)`), url, enabled, position, added_at, serialized `Source`
  (catalog, etag, last_modified, last_success, error), last_attempt_at, browse (`all` |
  `picked`), picked_at (when it was last switched to `picked`).
  Catalogs are also held in memory for queries.
- `app_prefs`: per (source_id, bundle_id): first_seen (recorded at startup, on add and on
  each refresh), shown (1 shown, 0 hidden, NULL: the source's `browse` decides), original
  (NULL: the App Store app with the same bundle id, `''`: none, else the App Store bundle id
  chosen by hand). Deleted with the source.
- `store_apps`: cached App Store lookups per (country, bundle_id): track_id (NULL when no
  App Store app has the bundle id), name, developer, version, released_at, icon_url,
  store_url, checked_at.
- `devices`: id (`ts:<StableID>` or `local:<random>`), kind, name, tailscale_name,
  os_version + os_version_source (`devicectl` > `installer` > `manual` > `safari`), model,
  devicectl_id (link to a paired CoreDevice), udid, person_id (the device's Tailscale user:
  their login, or `node:<StableID>`; `NULL` for browser devices on the Mac and devices from
  before Kiwi recorded users), first_seen, last_seen.
- `builds`: device-independent signed artifacts. id, source_id, bundle_id (original),
  version, build, release JSON, artifact_sha256, signed_bundle_id, url_scheme,
  identity_sha1, profile_uuids, expires_at, ipa_path, size, status
  (`queued|downloading|signing|packaging|ready|failed`), step text, progress (0..1),
  error, retryable (failed by a restart or a download/network error), dropped_entitlements,
  app_version (the signed IPA's `CFBundleShortVersionString`), created_at, finished_at.
  Reuse a ready build when the release, artifact, identity, profiles and signing settings
  are unchanged.
- `attempts`: token (128-bit random, URL-safe), build_id, device_id, created_at,
  manifest_at, download_started_at, download_completed_at, user_agent. One reusable
  token per (build, device).
- `installs`: per device and signed bundle id: source_id, bundle_id, build_id (nullable),
  version, build, origin (`kiwi` = download completed, `device` = seen by devicectl),
  verified (seen by devicectl), installed_at, last_seen_at, removed_at.
- `push_subscriptions`: endpoint (PK), device_id, p256dh, auth, created_at,
  last_success_at, failures.
- `notifications_sent`: dedupe key (PK), sent_at.
- `settings`: key/value JSON (see `Settings` in api.md). Defaults that are never saved
  follow the Mac: the App Store country is the region of its locale (`AppleLocale`, else
  `us`). `kiwi_builds` is off unless the owner turns it on; a Kiwi whose Kiwi Builds state
  already had apps added keeps it on.
- `meta`: also `owner_login` (the last owner Tailscale reported).
- Schema 5 dropped the family's `people`, `app_access`, `registrations`, `enrollments` and
  `builds.devices`.

### Device identification

Requests come from `tailscale serve` on loopback. When the peer is loopback and
`X-Forwarded-For` is present, take the client IP, resolve it with tailscaled's whois
(cache 10 min) and use device `ts:<StableID>`; Safari and the Home Screen app on the same
phone therefore share one device. When whois fails (error or timeout, as opposed to "not a
peer"), it is retried once, then the request fails with `503 device_unidentified` rather
than splitting the phone into a cookie device; requests from that address fail the same
way for 15 s without running whois again. Otherwise use (and set) a `kiwi_device`
cookie (`local:<random>`); the web app sends `/api/me` first so concurrent first requests
do not each create one, and cleanup deletes `local:` devices without installs, attempts or
push subscriptions that were not seen for 24 h. Update `last_seen`, the Tailscale name and,
from the User-Agent, the OS version (never downgrading the source priority).

Device rows are changed with targeted updates of the columns each operation owns (request
bookkeeping, rename, manual OS version, link/unlink, devicectl sync), never by writing back
a row read before an `await`, so concurrent changes are not reverted. The same holds for
sources: a refresh writes only the fetch results, so a source disabled meanwhile stays
disabled.

### The owner

Kiwi is its owner's alone. The owner is the Tailscale user who owns the Mac (LocalAPI
`status?peers=false`: the `User` of `Self.UserID`, asked at start and, while unknown, at most
every 30 s from the scheduler; saved in `meta`), unless `--owner` names them. Each request's
device records its Tailscale user from whois (`UserProfile.LoginName`) in `person_id`.

The `Ctx` extractor every API handler but `/api/health` takes answers `403 not_owner` for a
device whose user is not the owner (also while the owner is unknown, when only browser
devices on the Mac get in), so other people on the tailnet can't browse, install, sign or
change anything. The message names the owner, or says to pass `--owner` for a tagged Mac.
The scheduler likewise prepares updates and sends notifications only for the owner's devices.

### Android devices

Kiwi installs on Android only what Kiwi Builds made: a patched app the Android runner
(`automation/android.py`) signed with this Kiwi's own key. Nothing of the iOS path applies
to it (no `codesign`, profiles, expiry, manifest or URL scheme), and the model is kept
small:

- **Which device.** `devices.model` is `Android` (`kiwi_device::ANDROID_MODEL`) for a
  device that Tailscale reports as Android or whose browser sends an Android User-Agent,
  set only while no model is recorded. Chrome's User-Agent hides the OS version
  ("Android 10; K"), so `os_version` usually stays empty; Firefox reports it (source
  `android`, the priority of `safari`). The device profile's platform is `android`.
- **Which apps.** A release names its platforms; one that names none is an iOS release.
  Kiwi Builds publishes Android releases with platform `Android`, no `min_os` and the
  artifact `automation/artifacts/<sha256>.apk`. Browse leaves out the apps of the other
  platform; opened anyway, they are unavailable ("Made for Android devices", "Made for
  iPhone and iPad").
- **The build.** `builds::prepare_release` gives an Android release a `ready` build row
  that points at the artifact, after checking its digest again: no download, signing or
  expiry, and `signed_bundle_id` is the package itself. Kiwi Builds creates it at
  publication, so the app is installable at once. Android releases of any other source
  are refused.
- **The install.** `install_url` is `<public>/ota/<token>/app.apk`. The browser downloads
  the APK and the owner opens it with Android's installer. Kiwi cannot observe that step,
  so the completed download is the install, as for an IPA. There is no `open_url`.
- **Updates.** A newer revision is an `update` like any other; Android accepts it over the
  installed app because every build carries the same signature.

The Android runner needs no Mac-specific tool: it downloads a pinned Java runtime, patcher
and patch bundle, fetches the original app, verifies the publisher's signature over the
whole file with its own verifier (`automation/apk.py`, standard library only) and emits
the same events as `runner.py`. Details are in [kiwi-builds.md](kiwi-builds.md#android-apps).

### App actions

Computed per device; see `AppState` in api.md.

Target release = newest eligible release for the device's OS version (all releases when
unknown). Installed state = the `installs` row for `signed_bundle_id(original, team)` with
the app's own `source_id` and `bundle_id`. Several sources often ship different apps with
the same original bundle id (YouTubePlus, YouMod and uYou all use
`com.google.ios.youtube`), and they share the signed id. When it is installed from
another source, the app is treated as not installed (`get`, or `preparing`/`install`/
`installing`/`retry` for its own builds; installing it replaces the other app), is never
counted as an update, auto-prepared or notified, and `AppState.replaces` tells the UI
("Replaces <name> (<source>)").

- `unavailable`: app unavailable or no eligible release.
- `preparing`: a build for the target release is queued/running.
- `retry`: the latest build for the target failed.
- `open`: installed and `release_change` is `Same` (or `Older`/`Ambiguous`).
- `update`: installed and the target is `Newer`, or the installed build expires within
  30 days and a usable profile would give a later expiry (`update_reason = resign`).
- `install`: not installed and a ready build exists (then `install_url` is set).
- `get`: otherwise.
- `installing`: overrides `install`/`update` while an attempt for this device of a build of
  this app (source and bundle id) has a manifest fetch or started download within the last
  10 minutes and no completion.

When a ready build exists for an `update`, `install_url` is set so the Update button is a
direct `itms-services` link. `open_url` is `<url_scheme>://` when the installed build
carries Kiwi's scheme; installs not made by Kiwi have no `open_url`.

### Browse choices

Grab-bag sources list many apps the user does not want in Browse. Each source has a mode:
`all` (default) shows every app except those hidden by hand; `picked` shows only the apps
turned on by hand, so apps the source adds later stay out of Browse. Such an app, first
seen after `picked_at` and not turned on or off yet, is "new" (`is_new`, `Source.new_count`)
until the user decides. Switching to `picked` again restarts that. `GET /api/apps` leaves
hidden apps out unless `hidden=1` (the web app asks for them when searching or when the user
opens the hidden list). Hiding only affects Browse: installs, updates, auto-prepare,
notifications and My Apps are unchanged.

### Original apps

Most patched apps keep the bundle id of the App Store app they are based on (YouTubePlus,
YouMod and uYou all use `com.google.ios.youtube`). Kiwi looks those bundle ids up with
Apple's iTunes Search API (`https://itunes.apple.com/lookup?bundleId=a,b&country=us&entity=software`,
up to 20 ids per request, case-insensitive) in the storefront chosen in Settings
(`app_store_country`, by default the Mac's region), and caches the result in `store_apps`: apps found are
looked up again after 12 hours (for their current version), bundle ids without an App Store
app after 7 days. The user can link another App Store app by hand (searching
`/search?term=..&entity=software`, e.g. for Cercube's `com.google.ios.youtube.cercube`) or
say an app is not based on one. Requests go through kiwi-source's client (HTTPS, public
addresses only); this sends the bundle ids of the sources' apps to Apple.

`AppSummary.original` compares versions with `compare_numeric`: the signed IPA's version once
a ready build of the target release exists (some feeds list the tweak's version), else the
target release's. `behind` means the App Store has a newer version; versions that are not
numeric are `unknown`. Kiwi does not notify about the App Store getting ahead of a patch.
`AppDetail.other_versions` lists apps of other enabled sources with the same original (or,
without one, the same bundle id).

### OTA endpoints

- `GET /ota/<token>/manifest.plist`: `items[0].assets` = `software-package`
  (`<public>/ota/<token>/app.ipa`) and `display-image`/`full-size-image`
  (`<public>/ota/<token>/icon.png`, only when an icon is cached); metadata
  `bundle-identifier` (signed id), `bundle-version`, `kind=software`, `title`. Record
  `manifest_at`. Content-Type `application/xml`.
- `GET /ota/<token>/app.ipa`: streams the signed IPA with `Range` support,
  `Content-Type: application/octet-stream`, `Content-Length`. Record
  `download_started_at`. The download is complete when a full (non-`Range`) GET has written
  the last byte, or when the byte ranges fully served for the token (merged in memory,
  forgotten after an hour without requests) cover the whole file; fetching only the end of
  the zip is not an install. Then record `download_completed_at`, upsert `installs`
  (origin `kiwi`), and parse the installer User-Agent into the device record.
- `GET /ota/<token>/icon.png`: cached source icon (fetched with size limits).
- `GET /ota/<token>/app.apk`: an Android build's file, streamed like `app.ipa` with
  `Content-Type: application/vnd.android.package-archive` and a `Content-Disposition`
  file name. `manifest.plist` and `app.ipa` answer 404 for an Android build, and `app.apk`
  for an iOS one.

### Jobs and scheduler

- Builds run in background tasks: downloads at most 2 at a time, signing 1 at a time
  (in `spawn_blocking`), cancellation on shutdown, progress written to the DB. If saving a
  finished build fails (e.g. a full disk), it is retried once, then the build is marked
  failed and retryable with a minimal update, so it never stays active until a restart.
- Untrusted input is bounded before it is parsed: JSON5 feeds nested deeper than 128
  levels are rejected, and plists from an IPA (Info.plist, entitlements; at most 4 MiB)
  deeper than 64 levels or with more than a million values, so a hostile file cannot
  overflow the stack. Downloads may take 20 minutes plus 1 s per 128 KiB of their size
  (stalls fail after 30 s without data) and are flushed to disk before they are cached.
- A cached download is reused only while its SHA-256 matches its name; a damaged one is
  deleted and downloaded again, as is one that `sign_ipa` cannot extract.
- External tools have deadlines: `codesign` 10 minutes per call (a keychain prompt blocks
  it), `zip` 30 minutes, `security` 30 s, a whole signing scan 90 s (then signing is
  reported unavailable, keeping the known profiles).
- Scheduler tick every minute:
  - when Kiwi Builds is on, queue the added apps whose check is due (see
    [kiwi-builds.md](kiwi-builds.md));
  - refresh enabled sources older than `refresh_interval_minutes` (conditional requests;
    `last_attempt_at` is written before the fetch, so a fetch that crashes the server is
    not repeated right after the restart);
  - look up the original apps of enabled sources' apps that are not cached for the current
    storefront or are stale (see "Original apps"); after a failed lookup, wait 15 minutes;
  - every 10 minutes, rescan signing identities and profiles, and look for devicectl again
    while it is unavailable;
  - re-queue failed builds whose failure was a restart ("Interrupted by restart") or a
    download/network error: 1 minute after the first failure, 5 minutes after the second,
    at most 3 builds of a release per 24 h, and only within a day of the failure. Signing,
    profile and validation errors are not retried automatically (the app shows Retry);
  - for every device, find updates; when `auto_prepare_updates`, queue builds for them
    (a failure to queue is logged once per app and error, not every tick);
  - notify each update once per (device, signed id, version) and phase (when
    `notify_updates` and the device has a subscription): "available" when nothing will be
    prepared automatically (auto-prepare is off, queueing it failed, e.g. signing is
    unavailable, or its build failed and will not be retried), and "ready" when its build is
    ready, also after an "available" (separate dedupe keys; "available" is not sent after
    "ready"). The title names the app and the body shows the change: "YouTubePlus update
    ready" / "21.42.1 → 21.43.5 · Tap to install", "YouTubePlus update available" /
    "21.42.1 → 21.43.5" (builds are added when only they differ; "Version 21.43.5" when the
    installed version is unknown). A re-sign reads "YouTubePlus re-signed" / "Install
    before the current build expires", or "YouTubePlus expires soon" / "Re-sign it in Kiwi
    to keep using it". Tapping an update or expiry notification opens `/apps` (My Apps);
  - daily: expiry warnings at 30/7/1 days (`notify_expiry`);
  - every `devicectl_sync_minutes` when `devicectl_sync`: for each device linked to a
    reachable CoreDevice, list apps, mark Kiwi installs present (`verified`) or removed,
    record apps from enabled sources installed outside Kiwi (origin `device`), and take
    the OS version; auto-link when exactly one physical paired device and one Kiwi device
    exist. A Kiwi install completed in the last 10 minutes is neither marked removed nor
    given (or verified with) another listed version: iOS may still be replacing the old
    one;
  - cleanup: after 7 days, delete builds not referenced by a current install, except the
    newest build and the newest ready build of an app that is still in a catalog, unless
    that build has expired; delete unreferenced downloads after 14 days; clear `work/`;
    delete unused `local:` devices not seen for 24 h.

### Web push

VAPID keys in `<data>/vapid.json`. Payload is JSON `{title, body, url, badge, tag}`; the
service worker shows a notification for every push and sets the app badge. `Gone`
(404/410) removes the subscription. Permanent rejections (other 4xx except 408 and 429,
or a subscription the sender cannot use) increment `failures`; 10 in a row remove the
subscription. Transient failures (network errors, 5xx, 408, 429) are not counted. A
notification whose delivery failed is retried with exponential backoff per dedupe key
(1, 2, 4, ... minutes, at most an hour; in memory) instead of on every tick, and given up
24 hours after its first failure (the TTL of the push). An unreadable `vapid.json` is
moved to `vapid.json.corrupt-<time>` and replaced with a new key at start; the push
subscriptions made with the old key are deleted (each device subscribes again).

### Security

- Bind loopback only; Tailscale provides access control: `tailscale serve` publishes `/`
  with HTTPS to the tailnet only. `install.sh` refuses to install while Funnel is on for the
  Mac's name and never replaces another `/` mapping. Within the tailnet, only the owner's
  devices may use the API (see "The owner").
- Requests must name the public URL's host or the listen address
  (`127.0.0.1:<port>`, `localhost:<port>`, `[::1]:<port>`) in `Host`, and an
  `X-Forwarded-Host` must be the public host; otherwise `421 bad_host`. A DNS-rebinding
  page in a browser on the Mac therefore cannot reach the loopback port with a forged
  `X-Forwarded-For`.
- Mutating `/api` requests must carry `X-Kiwi: 1` and `Content-Type: application/json`
  (no CORS is allowed), which blocks cross-site form posts.
- Source and download URLs go through kiwi-source validation (https, no credentials,
  no private/loopback targets). Push endpoints must pass `endpoint_allowed`.
- Paths are never built from user input except through validated ids.
- The signing identity and the Apple sign-in for Kiwi Builds stay on the Mac; the API never returns credentials or private keys.

## Web app (web/)

Vite + Preact + TypeScript, hand-written CSS in the style of iOS (light and dark via
`prefers-color-scheme`, safe-area insets, a floating bottom tab bar, grouped card lists).
Surfaces are calm greys (true black in dark mode) with one accent, Evergreen (`#1e6b4d`,
`#5bbf94` in dark mode), for actions: buttons, links, switches and the current tab. The rest
of the colour comes from the apps: their source's `tintColor` (or a palette colour picked from
the name when there is none) makes their icons glow, fills their download ring, washes their
update card and paints their Fresh card and the top of their page (darkened until white text
on it reads at 4.5:1; `web/src/lib/tint.ts`). Tab pages have a soft kiwi glow (green, teal,
a touch of gold) behind the large title; titles use SF Pro Rounded. Amber marks warnings
(expiring within a month), red errors (failed, expiring within a week). Two button styles:
filled for the one tap that installs (Install, Update when the build is ready), grey with
accent text for everything else; on an app's colour these become white and frosted. Settings
rows have white glyphs on coloured tiles, like iOS.
Four tabs; routes: `/` (Browse), `/apps` (My Apps), `/sources`, `/settings`, `/sources/:id`
(a source's page), `/builds` and `/builds/:app` (Kiwi Builds, under Sources; earlier
`/settings/builds` and `/automation` links still work), `/settings/signing`,
`/settings/device`, `/settings/notifications` and `/app/:sourceId/:bundleId`. `/updates` and `/library` (the earlier Updates and Library tabs,
still used by older notifications) open My Apps.

Warnings appear where they are fixed rather than as banners on every screen: an expiring
app in My Apps (its re-sign is an update, so the badge counts it), a failed source in Sources,
signing problems and an unknown iOS version in Settings. Sources and Settings get a dot on
their tab (amber, or red when signing is impossible); only warnings that stop every install
(no identity or profile) also show on Browse.

- Browse: the large title, search, then "Fresh This Week": a shelf of cards in each app's
  colour with its button, for releases from the last week that still need something (updates
  first, then the newest; picked once per visit so a card stays while its app installs). Then
  "All Apps" with the count and two capsules, "All Sources ⌄" (a native picker over the
  enabled sources) and "Name ⌄" / "Recently Updated", above the list. Rows have
  two lines: the name and one detail, which is the first that applies: what a running or
  failed action is doing ("Signing 13 of 23…", "Couldn't prepare: …"), the expiry of a
  re-sign, the version change of an update, "Replaces <name> (<source>)", why it is
  unavailable, "Based on <App Store name>", or the subtitle. The button shows the rest of the
  state. Under the list, "N apps hidden · Show" lists the hidden apps dimmed, each with a Show
  button; searches list hidden matches in a "Hidden" group after the others.
- App page: the top takes the app's colour and fades into the page: a centred icon, name,
  subtitle, developer and source, a wide action button with the version change
  ("21.43.5 → 21.44.0") and its status (step, error, what it replaces, why it is
  unavailable; problems on a dark chip); the back and ••• buttons float on tinted glass.
  Below: an info strip (version, size, minimum iOS, signature time left or source; with an
  original, its App Store version replaces the minimum iOS, in amber when ahead), then
  what's new, screenshots and the description; Kiwi Builds apps link to their patches
  and schedule; "Original App" (the App Store
  app with a link, this app's and the App Store's version and date, how it was matched; or
  "Not in the App Store" with "Link…"), other versions of the same app; then the installed
  build, version history (install a specific version), privacy descriptions and Information
  (entitlements, with the ones signing removes). "•••" in the bar: "Reinstall" (when the
  action is `open` and there is a target release), "Hide from Browse"/"Show in Browse" and
  "Change Original App…", a sheet with "Match by Bundle ID", "Not Based on an App Store App"
  and an App Store search.
- Reinstall: without devicectl Kiwi can't see an app deleted on the iPhone, so it keeps
  showing Open. Reinstall prepares the target release right away (a ready build is reused),
  replaces the button with Reinstall (the install link), Installing and Retry, and ends when
  the install's `installed_at` changes (a completed download records it again) or on
  "Cancel Reinstall".
- Action button state machine: Get -> Preparing (step + progress, polling the build) ->
  Install (an `<a href=itms-services...>` that the user taps; iOS requires a real tap)
  -> Installing (polling) -> Open (`<a href=url_scheme://>`). Update behaves like Get
  for a newer version. Retry on failure, with the error.
- My Apps (tab badge = update count) replaces separate Updates and Library tabs, which
  listed mostly the same apps as Browse: "Updates" (`/api/updates`: version change and size,
  or the expiry of a re-sign; two lines of release notes; "Update All", which prepares all;
  each install still needs a tap; one card per app, washed with its colour), then "Installed"
  (`/api/library` minus the pending updates: version and signature expiry, installed outside
  Kiwi, source removed; no button when Kiwi can't open the app). The outline round each icon
  Kiwi signed is a signing timer: it fills with the share of the signature left, green, amber
  within a month, red within a week. Edit shows "Forget" on every install (one row per install,
  keyed by signed bundle id). "Check Now" refreshes the sources.
- Sources: "Built on Your Mac" (Kiwi Builds, while it is on: its apps and how many more can be added), then
  the AltStore sources, one row each (apps and last update, "4 of 10 apps in Browse", "N new",
  warnings, off, or the error with Retry), then "Apps from Links" (IPA links and download
  pages), opening the source's page; **+** adds a source, download page or IPA by URL in a
  sheet (validated by the server). A source's page (`/sources/:id`): on/off, URL,
  refresh, the error and diagnostics, "All Apps" or "Apps I Pick" with a switch per app (new
  ones marked New), and Remove Source. Kiwi Builds (`/builds`) lists the added apps, the ones
  to add, its source's settings and the Mac's setup.
- Another person's device: once `/api/me` answers `403 not_owner`, "Not Your Kiwi" with the
  server's message replaces the tabs.
- Settings is a short index: Kiwi's card (deep green with a slowly turning kiwi slice, the
  version, public host, and whether signing is ready, with a pulse, or expiring), this iPhone (iOS version, amber when unknown) and Notifications (on or
  off), the Mac (Signing with its expiry, App Store country),
  Updates (source refresh interval, auto-prepare updates, notify on updates/expiry, remove
  watch apps) and Kiwi Builds (a switch that asks for confirmation before turning it on). Sub-pages: Signing (ready or why not, the certificates, the profiles named by
  App ID with expiry and entitlements, Rescan), This iPhone (name, iOS version and where it
  came from, model, Tailscale name, linked Mac device, Sync Now, devicectl sync, device ID)
  and Notifications (enable, test, disable).
- PWA: `manifest.webmanifest` (`display: standalone`, scope `/`), apple-touch-icon and
  icons (a kiwi fruit), `sw.js` (network-first app shell, never caches `/api` or `/ota`,
  push + notificationclick + app badge; when the server answers 5xx, e.g. `tailscale serve`'s
  502 while Kiwi restarts, it serves the cached shell so the app can show its own error),
  "Add to Home Screen" hint when not standalone.
- Requests time out (30 s, 2 min for refreshes, syncs and signing scans) so a stuck request
  cannot leave a screen loading forever.
- Badge: on load and focus, set `navigator.setAppBadge(updates)` or clear it.

## Not in scope yet

- Creating App IDs, app groups or profiles automatically (Kiwi already prefers an exact
  App ID profile when Xcode has one).
- Silent installs and auto-update through devicectl (the API exists; needs pairing and
  testing with the real phone).
