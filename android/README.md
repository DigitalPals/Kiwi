# Kiwi for Android

A native Android client for Kiwi. It talks to the same JSON API as the web app
([docs/api.md](../docs/api.md)), installs Kiwi Builds' Android apps with Android's
PackageInstaller, tells Kiwi which of its packages are installed, checks for updates in the
background and installs them without asking where Android allows it.

Screens: Connect, Browse, an app's page, My Apps, Builds (Kiwi Builds) and Settings, in
Material 3 with Kiwi's Evergreen accent (light and dark) and each app's own colour.

- Application id `io.github.digitalpals.kiwi`, minSdk 26 (Android 8.0), target and compile
  SDK 37.
- Kotlin, Jetpack Compose, Material 3, Navigation, WorkManager, DataStore,
  kotlinx-serialization, OkHttp and Coil. No Google Play Services, Firebase or analytics.

## Build

The project is a standard Gradle build (`android/`, Kotlin DSL, version catalog in
`gradle/libs.versions.toml`, wrapper committed). With an Android SDK and JDK 17 or later:

```sh
cd android
./gradlew test assembleDebug assembleRelease
```

Without an SDK, build in a container (needs Docker):

```sh
android/scripts/build-in-docker.sh                 # test, debug APK and unsigned release APK
android/scripts/build-in-docker.sh test            # any Gradle tasks
```

The script uses `ghcr.io/cirruslabs/android-sdk:36` pinned by digest, caches Gradle and the
SDK packages the build adds in `android/.cache/` (ignored by git) and runs Gradle as your
user. The build downloads Gradle, the dependencies and the compileSdk platform, so the
container needs internet access. Where the local containers have no internet access (or
there is no Docker), `KIWI_BUILD_HELPER=<command>` hands the build to a helper of your own,
for example one that runs the container on another machine over SSH: the sources go to it
on stdin and `app/build/outputs`, `reports` and `test-results` come back. The script's header
describes how the helper is called.

Outputs:

- `app/build/outputs/apk/debug/app-debug.apk`, signed with the debug key of the machine that
  built it (in the container: `android/.cache/home/.android/debug.keystore`). Installable
  for testing; updates only install over a build with the same key.
- `app/build/outputs/apk/release/app-release-unsigned.apk`, shrunk with R8. There is no
  signing configuration in the repository: release APKs are signed by whoever distributes
  them (Kiwi or a release process), with `apksigner` and a key kept outside the repository.
  Every release of the app must be signed with the same key, or Android refuses the update.

The unit tests (`./gradlew test`) run on the JVM: API models and the Kiwi Builds and community endpoints,
the User-Agent, server addresses and `kiwi://connect` links, the app button's states, the app
colours and the "Fresh this week" shelf (ported from the web app, with its test cases), Kiwi
Builds' and the community section's wording (failure actions, notices, candidates), community
search paging, waiting for Kiwi Builds and community builds to install them, the package report (against a fake Kiwi), the install flow (one job per app,
"Update all", cancelling, the APK checks; with fakes for PackageInstaller and PackageManager),
downloads (resuming, stale partial files, redirects, cancelling), install outcomes, the
background install conditions, and update and build notifications. Nothing needs an
emulator.

`testDebugUnitTest` also renders every screen with made-up data (`ui/preview/PreviewData.kt`)
in light and dark, in Robolectric with native graphics and Roborazzi, to
`app/build/outputs/roborazzi/*.png` (C1–C6 of the design, the Builds sheets, the community
section in its states (`c6-community*`: Linux, Mac, loading, error, the install and app
sheets, candidates, search), the launcher icon). They are records to look at, not golden images: the test fails only when a screen can't
be composed. Every screen also has Compose previews for Android Studio.

The launcher, notification and in-app logo icons are vector drawables generated from the web
app's icon: `python3 android/scripts/make-icons.py web/public/icons/icon.svg
android/app/src/main/res`. The launcher icon is adaptive (gradient background, the kiwi slice
inside the safe zone) with a monochrome layer for themed icons.

## Connecting

The app needs no login: Kiwi identifies the phone by its Tailscale node, exactly like the
browser, so Chrome and the app on the same phone are one Kiwi device. The user enters the
Kiwi address (`https://<name>.<tailnet>.ts.net`) or opens a link
`kiwi://connect?url=<https address>` (an https App Link is not possible for arbitrary
hosts). A link only fills in the address; the user still taps Connect.

Connecting calls `GET /api/health` (is this Kiwi?) and `GET /api/me` (may this phone use
it?), and only then saves the address (DataStore). Kiwi's own errors from `/api/health` (its
host check covers it) are shown as they are; any other answer means "This doesn't look like a
Kiwi server". Errors:

- Network failures: "Can't reach Kiwi at <host>. Is Tailscale on?"
- `403 not_owner`: "Not Your Kiwi" with Kiwi's message, which names the owner.
- `503 device_unidentified`: Kiwi couldn't identify the phone through Tailscale.
- `421 bad_host`: the address isn't the one `tailscale serve` publishes.

Every request to Kiwi sends the User-Agent
`Kiwi-Android/<versionName> (<versionCode>; Android <release>; SDK <int>; <manufacturer> <model>)`,
from which Kiwi marks the device Android and reads its Android release. Requests for icons
on other hosts only send `Kiwi-Android/<versionName>`. API requests and APK downloads don't
follow redirects (Kiwi sends none, and nothing the phone sends or installs may come from
another host), and a network security config forbids plain HTTP, also on Android 8.x where it
would otherwise be allowed.

## How it works

```
app/src/main/kotlin/io/github/digitalpals/kiwi/
  api/        Kiwi's JSON types, the HTTP client (OkHttp), errors, User-Agent, server address
  data/       DataStore: the server address and the app's own switches
  packages/   PackageManager scan and the PUT /api/me/packages report
  install/    APK download (Range resume), PackageInstaller sessions, the install flow
  work/       WorkManager update check, notifications
  ui/         Compose screens (Connect, Browse, App, My Apps, Builds, Settings), theme and
              icons, app colours, the Fresh shelf, Kiwi Builds wording, app button states
```

- **Design.** Material 3 with a fixed scheme seeded from Evergreen (`#1E6B4D`, dark `#5BBF94`);
  no dynamic colour, so the brand stays. Roboto (the system font), Material 3 app bars and a
  navigation bar with Browse, My Apps (badge: the number of updates, Kiwi's own included),
  Builds and Settings. Filled buttons only install or update in one tap; everything else is
  tonal. Apps bring their own colour (the source's tint, else one picked from the name, as in
  the web app): tinted icon glows, the top of an app's page with a white button that fills
  while Kiwi downloads, update cards washed with the app's colour, and the "Fresh this week"
  carousel (Material 3's multi-browse carousel) of releases from the last week.

- **Installed packages.** The app reports only the packages Kiwi asks about: its own, every
  Android app Kiwi offers (`GET /api/apps?hidden=1`, hidden ones included, filtered to
  platform `android`) and every install Kiwi recorded for the phone (`GET /api/library`,
  orphaned ones included, by `signed_bundle_id`). `PackageScanner` looks each of them up by
  name with its version, signing certificates (SHA-256 of each DER certificate: all signers of
  a multi-signer APK, else the signer and its rotation history) and installer of record; the
  phone's other apps are never read or sent. Without both lists nothing is sent, since Kiwi
  marks a recorded install whose package a report lacks as removed. The report goes to
  `PUT /api/me/packages` when the app starts, after each install and, while it runs, after a
  package was added, replaced or removed. Kiwi matches it against its Android releases and
  its signing certificate (verified install, or `foreign_install` for a copy with another
  signature).
- **Installing.** For `install`/`update`/`open` (reinstall) the app uses the state's
  `install_url`; otherwise it asks Kiwi to prepare the build (`POST .../prepare`) and waits
  until it is ready. It downloads the APK from Kiwi only (the URL must be on the server's
  host) into the app's cache, resuming a partial file with `Range`. A partial file belongs to
  one URL, version code and size (Kiwi's own app has one URL for every version), and is
  started over when the server's file no longer has the size Kiwi announced. Before
  installing it checks that the file is the package Kiwi offered, in the version Kiwi offered
  (the build's `build`, which is the version code for Android, or `android_app.version_code`),
  signed with Kiwi's Android key (`Me.host.android_signer_sha256`, when Kiwi knows it, for
  Kiwi Builds' apps and Kiwi itself), not older than the installed copy and signed like it; a
  different signature than the installed copy's stops with "Another <Name> is installed with
  a different signature" and a button that opens Android's uninstall dialog. Then it writes
  the APK into a PackageInstaller session and commits it. A session is abandoned whenever
  the install fails or is cancelled, also while the APK is still being copied into it.
- **One job per app.** Every install, from a screen or from the background worker, registers
  one job per app (and one for Kiwi itself) before it starts; asking again while it runs
  joins that job, and Cancel on the app's button stops it, also when the worker started it.
  "Update all" in My Apps starts the updates that aren't running yet and Kiwi's own update
  only once all of them have finished, since installing it restarts the app.
- **Confirmation.** On Android 12 and later every session asks for
  `USER_ACTION_NOT_REQUIRED`. Android (PackageInstallerSession's user action rules) honours
  this when the app holds `UPDATE_PACKAGES_WITHOUT_USER_ACTION` and the session updates a
  package that has an update owner (Android 14+) that is Kiwi, or, without an update owner, a
  package whose installer of record is Kiwi or Kiwi itself (a self-update, also when the
  browser installed Kiwi first). The APK must also target a recent SDK (Kiwi targets the
  newest), and a repeated update of one package within a short time asks anyway.
  `PackageScanner.canUpdateSilently` follows these rules; whatever Android decides, an
  install that needs a confirmation still gets one.
- **Update ownership.** On Android 14 and later every session also calls
  `setRequestUpdateOwnership(true)`. Android applies it to the first install of a package
  (it is a no-op for updates): Kiwi becomes the app's update owner, so another installer, such
  as a store, can't replace Kiwi's build of it without the user's confirmation, and Kiwi's own
  updates of it stay silent. An app installed before (by Kiwi or anyone else) keeps its
  current owner until it is removed and installed again. Otherwise the session reports `STATUS_PENDING_USER_ACTION`: the app
  shows Android's confirmation at once while it is on screen, or posts a notification that
  opens it when it is in the background. The final status maps to success, cancelled, a
  signature conflict or a failure with Android's message, and the app reports its packages
  again. While its own job runs, the button offers Cancel (which abandons the session), in
  case a confirmation was dismissed without Android reporting it.
- **Kiwi itself.** When `Me.android_app.version_code` is newer than the running app, My Apps
  offers the update (`android_app.install_url`), installed the same way. The background
  check installs it silently on Android 12+ (see Confirmation) and otherwise, or when that
  doesn't finish, notifies it once per version.
- **Background updates.** WorkManager runs a check about every two hours while there is a
  network (any network): it reports the packages, reads `GET /api/updates`, notifies each
  update once (per app and target release) and, with "Install updates automatically" (on by
  default), installs the updates Android allows without a confirmation, but only on an
  unmetered network (Wi-Fi, Ethernet; under Tailscale's VPN a validated unmetered network
  counts) with a battery that isn't low (charging or above 15 %), checked before each
  install. Updates that would need a tap, or that wait for Wi-Fi, are notified instead; an
  update installed later removes its notification. The periodic check and the build watch
  are separate works; only one of them installs at a time. A check that cannot reach Kiwi
  (Tailscale off) waits for the next period.
- **Opening apps.** Kiwi's API has no `open_url` for Android; the app opens installed apps
  with their launcher intent.
- **Kiwi Builds.** The Builds tab lists every Android recipe of `/api/automation` in a "Kiwi
  Builds" section (docs/api.md, "Kiwi Builds (automation)"), added or not, polled every 3
  seconds while a job runs, else every 30 seconds, while the tab is open. Each row has one
  button: **Install** before the first build; "Waiting — N ahead" (the job's `ahead`) or the
  running stage with a progress bar while it builds; **Retry** with the reason when the first
  build failed; and the published build's usual Install, Update or Open (and its install's
  progress) once there is one. Install first shows a sheet: what Kiwi does and that the first
  build takes about 3–8 minutes, the recipe's limitations, a copy of the package on the phone
  that a Kiwi build couldn't replace (another signature than `Me.host.android_signer_sha256`,
  or `foreign_install`; it offers to uninstall it, before minutes are spent building), and
  "Customise patches" (the patches and schedule). **Build and install** saves the wish to
  install on the phone (DataStore, `pending_build_installs`), then adds the app (`POST
  .../add`, idempotent on the server) or checks it again, so a second tap never queues a
  second build. An app that already has a published build just installs it. While Kiwi is in
  the foreground, `install/BuildInstalls.kt` looks every 5 seconds and starts the install of
  a ready build (Android's confirmation); in the background the update worker notifies
  "<App> is ready — tap to install" (it opens Builds, which installs it), since Android
  doesn't let an app open the installer from there; with notifications denied the install
  starts the next time Kiwi is opened. The wish ends when the install starts, the build fails
  (its reason in the row and a notification), is cancelled, the app is removed or Kiwi
  Builds is turned off. Added apps keep the ⋮ menu (Build now / Check now, Rebuild, Cancel,
  Patches and schedule, Remove) and the Schedule section ("Build now" checks every added app
  that has no job). iOS recipes are not shown. While Kiwi Builds is off (`409 builds_off`),
  the rows come from the app's own copy of the Android recipes (`KiwiBuilds.OFF_APPS`, checked
  against `automation/android.json` by a server test); Install, or the card above them, asks
  to turn it on (`PATCH /api/settings {"kiwi_builds": true}`) after a confirmation of what it
  does, as the web app's Settings does, and then continues with the app's sheet. An app's page
  shows its patches when it comes from Kiwi Builds (or a community app).
- **Community apps.** Below Kiwi Builds, a "Community" section shows apps patched by
  independent authors of the Morphe community (`/api/community`, docs/api.md), each marked
  "Community · not reviewed": first the owner's community apps, with the same rows as Kiwi
  Builds' (queue position, stage and progress, install when ready, Open or Update, a failed
  build and its one action), then "Popular" (up to 30), then a search sheet
  (`/api/community/search`, 300 ms after the last keystroke, 30 results a page, loading the
  next page as the end comes into view; `ui/CommunitySearch.kt`). The phone never talks to
  the Morphe index, GitHub or Google Play itself; icons come through Kiwi's art proxy. Install
  opens a sheet: the first build's time, a copy of the app on the phone that Kiwi's build
  can't replace (step 1's signature check: community builds keep the original package, so the
  Play Store app must be uninstalled first, before minutes are spent building), and what to
  know (third-party code Kiwi hasn't reviewed, naming the bundle once Kiwi picked one; it
  replaces the Play Store app; Google sign-in and notifications may not work). **Build and
  install** posts `/api/community/:id/install` and waits like Kiwi Builds' apps
  (`BuildInstalls` reads the owner's community apps from `/api/community` when one is waiting,
  and keeps waiting when that list can't be read). An added app's sheet (tap or ⋮) shows the
  bundle (repo, author, tag, picked automatically or chosen by you), its applied patches, an
  offered update ("New patches: vX → vY": Update approves it and rebuilds, `POST .../update`),
  Cancel build and Remove. "Choose other patches" lists `/api/community/:id/candidates`
  (recommended first; author, stars, last update, patches, versions, and why one failed);
  picking one installs with `{"repo": ...}`. A failed build maps the job's `error_kind` to one
  action: `retry` → Retry, `other_bundle` → Choose other patches, `unavailable` → an
  explanation and Remove. Community apps have no schedule or patch switches, and "Build now"
  leaves them out. When Kiwi can't build them (`available: false`, e.g. on a Mac) the rows
  and search still show, with Kiwi's `reason` in a calm card and no Install buttons; before
  the index's first fetch (`index: null`) the card says it is loading. With Kiwi Builds off
  the section asks to turn it on. On a Kiwi without the catalogue (404) it is hidden.
- **Build notifications.** With "Notify when a build finishes" (on by default) each update
  check also reads `/api/automation` (and the owner's community apps) and notifies builds that finished or failed since the
  last look (the first look only remembers what it saw); a ready build opens the app's page,
  a failed one the Builds tab. Builds of apps waiting to be installed are notified even with
  the switch off, as "<App> is ready — tap to install". A ready build of an app that is installed is left to the
  update notification (or the silent update). While a build runs,
  the app checks again 5 minutes later, so a build is notified (and installed, where Android
  needs no confirmation) soon after it finishes.
- **Settings.** The server, this phone, "Install updates automatically" (on by default; on
  Wi-Fi with a battery that isn't low),
  Android's notification settings for the app's channels (Updates, Kiwi Builds, Installs),
  the Kiwi app's own update, "Show setup again" (the Connect screen with the current address)
  and Disconnect. Connecting marks the device as set up on Kiwi (`PATCH /api/me
  {"onboarded": true}`), since the web app's onboarding isn't needed in the native app.

## Permissions

| Permission | Why |
|---|---|
| `INTERNET` | Talk to Kiwi over the tailnet. |
| `QUERY_ALL_PACKAGES` | Look up the packages Kiwi asks about (its Android apps, which change with Kiwi's catalogue, and the app itself), so Kiwi knows what is installed and with which signature. The app looks them up by name and never lists or reports the phone's other apps. It is not distributed through Google Play, whose policy restricts this permission. |
| `REQUEST_INSTALL_PACKAGES` | Install APKs with PackageInstaller. The user also has to allow "Install unknown apps" for Kiwi once; the app links to that setting. |
| `UPDATE_PACKAGES_WITHOUT_USER_ACTION` | Android 12+: update apps Kiwi installed without a confirmation (`USER_ACTION_NOT_REQUIRED`). |
| `ENFORCE_UPDATE_OWNERSHIP` | Android 14+: become the update owner of the apps Kiwi installs (`setRequestUpdateOwnership`). |
| `REQUEST_DELETE_PACKAGES` | Open Android's uninstall dialog for a copy of an app that has another signature. The app never uninstalls anything itself. |
| `POST_NOTIFICATIONS` | Android 13+: update notifications and installs waiting for a tap. Asked once after connecting, and from Settings. |
| `ACCESS_NETWORK_STATE` | Background installs wait for an unmetered network (WorkManager needs it too). |
| `WAKE_LOCK`, `RECEIVE_BOOT_COMPLETED`, `FOREGROUND_SERVICE` | Added by WorkManager for its scheduling (the app itself starts no foreground service). |
| `<applicationId>.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION` | Added by AndroidX Core: keeps other apps out of the package-change receiver on Android versions without `RECEIVER_NOT_EXPORTED`. |

The app keeps no backup or device transfer (`allowBackup="false"`, data extraction rules
that exclude everything): it stores only the server address and its switches.

## Not verified yet

The app has been built, unit-tested and its screens rendered on the JVM; it has not run on a
device or emulator yet. To check on a phone: connecting (and the error pages), the package
report, a first install (confirmation), update ownership on Android 14+, a silent update of an
app Kiwi installed, the signature-conflict path, the self-update (silent on Android 12+ when
Kiwi was first installed from the browser), background checks and notifications (updates and
builds), background installs waiting for Wi-Fi under Tailscale's VPN, Cancel during a stalled
download and during the copy into a session, connecting while the screen rotates, the Builds tab against a real Kiwi (Install and its sheet, the wait
for a first build in the foreground and in the background, its notification, the conflict
warning before a build, Retry, patches, schedule, Build now, cancel, remove, turning Kiwi
Builds on from Install), the community section against a real Kiwi (Install with the Play
Store app installed, search paging, choosing other patches, Update, the Mac's browse-only
card), the carousel and pull to refresh
by touch, the bottom sheets and menus, and the themed launcher icon.
