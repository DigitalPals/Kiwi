# Kiwi for Android

A native Android client for Kiwi. It talks to the same JSON API as the web app
([docs/api.md](../docs/api.md)), installs Kiwi Builds' Android apps with Android's
PackageInstaller, tells Kiwi which packages are installed, checks for updates in the
background and installs them without asking where Android allows it.

Status: foundation. The screens work but are plain Material 3; their final design follows.

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
container needs internet access. On a host with the `agent-resource` helper it runs the
container through it (bounded CPU and memory, removed when the build ends). Where the local
containers have no internet access, `KIWI_BUILD_HELPER=<command>` runs it through an
`agent-resource`-compatible helper on another machine: the sources go to it on stdin and
`app/build/outputs`, `reports` and `test-results` come back (nothing is cached there).

Outputs:

- `app/build/outputs/apk/debug/app-debug.apk`, signed with the debug key of the machine that
  built it (in the container: `android/.cache/home/.android/debug.keystore`). Installable
  for testing; updates only install over a build with the same key.
- `app/build/outputs/apk/release/app-release-unsigned.apk`, shrunk with R8. There is no
  signing configuration in the repository: release APKs are signed by whoever distributes
  them (Kiwi or a release process), with `apksigner` and a key kept outside the repository.
  Every release of the app must be signed with the same key, or Android refuses the update.

The unit tests (`./gradlew test`) run on the JVM: API models, the User-Agent, server
addresses and `kiwi://connect` links, the app button's states, the package report, install
outcomes and update notifications. Nothing needs an emulator.

## Connecting

The app needs no login: Kiwi identifies the phone by its Tailscale node, exactly like the
browser, so Chrome and the app on the same phone are one Kiwi device. The user enters the
Kiwi address (`https://<name>.<tailnet>.ts.net`) or opens a link
`kiwi://connect?url=<https address>` (an https App Link is not possible for arbitrary
hosts). A link only fills in the address; the user still taps Connect.

Connecting calls `GET /api/health` (is this Kiwi?) and `GET /api/me` (may this phone use
it?), and only then saves the address (DataStore). Errors:

- Network failures: "Can't reach Kiwi at <host>. Is Tailscale on?"
- `403 not_owner`: "Not Your Kiwi" with Kiwi's message, which names the owner.
- `503 device_unidentified`: Kiwi couldn't identify the phone through Tailscale.
- `421 bad_host`: the address isn't the one `tailscale serve` publishes.

Every request to Kiwi sends the User-Agent
`Kiwi-Android/<versionName> (<versionCode>; Android <release>; SDK <int>; <manufacturer> <model>)`,
from which Kiwi marks the device Android and reads its Android release. Requests for icons
on other hosts only send `Kiwi-Android/<versionName>`.

## How it works

```
app/src/main/kotlin/io/github/digitalpals/kiwi/
  api/        Kiwi's JSON types, the HTTP client (OkHttp), errors, User-Agent, server address
  data/       DataStore: the server address and the app's own switches
  packages/   PackageManager scan and the PUT /api/me/packages report
  install/    APK download (Range resume), PackageInstaller sessions, the install flow
  work/       WorkManager update check, notifications
  ui/         Compose screens (Connect, Browse, App, My Apps, Settings), app button states
```

- **Installed packages.** `PackageScanner` lists every installed package with its version,
  signing certificates (SHA-256 of each DER certificate: all signers of a multi-signer APK,
  else the signer and its rotation history) and installer of record. The app sends the
  user-installed packages (and updated system apps) to `PUT /api/me/packages` when it
  starts, after each install and, while it runs, after a package was added, replaced or
  removed. Kiwi matches them against its Android releases and its signing certificate
  (verified install, or `foreign_install` for a copy with another signature).
- **Installing.** For `install`/`update`/`open` (reinstall) the app uses the state's
  `install_url`; otherwise it asks Kiwi to prepare the build (`POST .../prepare`) and waits
  until it is ready. It downloads the APK from Kiwi only (the URL must be on the server's
  host) into the app's cache, resuming a partial file with `Range`. Before installing it
  checks that the file is the expected package, not older than the installed one, and
  signed like the installed copy; a different signature stops with "Another <Name> is
  installed with a different signature" and a button that opens Android's uninstall
  dialog. Then it writes the APK into a PackageInstaller session and commits it.
- **Confirmation.** On Android 12 and later every session asks for
  `USER_ACTION_NOT_REQUIRED`. Android honours this for an update of an app whose installer
  of record (or, on Android 14, update owner) is Kiwi, so apps Kiwi installed update
  silently afterwards. Otherwise the session reports `STATUS_PENDING_USER_ACTION`: the app
  shows Android's confirmation at once while it is on screen, or posts a notification that
  opens it when it is in the background. The final status maps to success, cancelled, a
  signature conflict or a failure with Android's message, and the app reports its packages
  again. While its own job runs, the button offers Cancel (which abandons the session), in
  case a confirmation was dismissed without Android reporting it.
- **Kiwi itself.** When `Me.android_app.version_code` is newer than the running app, My Apps
  offers the update (`android_app.install_url`), installed the same way.
- **Background updates.** WorkManager runs a check about every two hours while there is a
  network: it reports the packages, reads `GET /api/updates`, notifies each update once
  (per app and target release) and, with "Install updates automatically" (on by default),
  installs the updates Android allows without a confirmation. Updates that would need a
  tap are notified instead. A check that cannot reach Kiwi (Tailscale off) waits for the
  next period.
- **Opening apps.** Kiwi's API has no `open_url` for Android; the app opens installed apps
  with their launcher intent.

## Permissions

| Permission | Why |
|---|---|
| `INTERNET` | Talk to Kiwi over the tailnet. |
| `QUERY_ALL_PACKAGES` | See every installed package, so Kiwi knows what is installed and with which signature. The app is not distributed through Google Play, whose policy restricts this permission. |
| `REQUEST_INSTALL_PACKAGES` | Install APKs with PackageInstaller. The user also has to allow "Install unknown apps" for Kiwi once; the app links to that setting. |
| `UPDATE_PACKAGES_WITHOUT_USER_ACTION` | Android 12+: update apps Kiwi installed without a confirmation (`USER_ACTION_NOT_REQUIRED`). |
| `REQUEST_DELETE_PACKAGES` | Open Android's uninstall dialog for a copy of an app that has another signature. The app never uninstalls anything itself. |
| `POST_NOTIFICATIONS` | Android 13+: update notifications and installs waiting for a tap. Asked once after connecting, and from Settings. |
| `ACCESS_NETWORK_STATE`, `WAKE_LOCK`, `RECEIVE_BOOT_COMPLETED`, `FOREGROUND_SERVICE` | Added by WorkManager for its scheduling (the app itself starts no foreground service). |
| `<applicationId>.DYNAMIC_RECEIVER_NOT_EXPORTED_PERMISSION` | Added by AndroidX Core: keeps other apps out of the package-change receiver on Android versions without `RECEIVER_NOT_EXPORTED`. |

The app keeps no backup or device transfer (`allowBackup="false"`, data extraction rules
that exclude everything): it stores only the server address and its switches.

## Not verified yet

The app has been built and unit-tested; it has not run on a device or emulator yet. To
check on a phone: connecting (and the error pages), the package report, a first install
(confirmation), a silent update of an app Kiwi installed, the signature-conflict path, the
self-update, background checks and notifications.
