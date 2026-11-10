# Kiwi Builds

Kiwi Builds patches App Store apps with open-source tweaks and signs them on your Mac:
**YouTube** (YouMod), **YouTube Music** (YTMusicUltimate), **Twitch** (TwitchAdBlock),
**X** (NeoFreeBird) and **Reddit** (RedditFilter). Signed releases appear in the built-in
**Kiwi Builds** source and install and update like any other app in Kiwi.

It also builds **Android** apps: YouTube, YouTube Music and Reddit, patched with
[Morphe](https://morphe.software)'s open-source patches and signed with a key of your own.
They need no Apple account, Xcode or Mac setup; see [Android apps](#android-apps). On a Kiwi
that runs on Linux or in Docker, Kiwi Builds builds these Android apps only: iOS apps need a
Mac, and the page shows a **macOS** setup check that is never ready.

It is optional and off by default. Kiwi itself works without it; see the
[README](../README.md) and [installation guide](install.md) first.

## Before you turn it on

Kiwi Builds downloads App Store apps with your own Apple account, decrypts them on your
Mac with [UnfairPlay](https://github.com/DrWhax/unfairplay) and injects open-source
tweaks into them. Modified apps may violate the apps' terms of service; X's terms forbid
modified clients outright. Use it only for apps you may use, on your own devices, at your
own risk. Kiwi publishes nothing outside your tailnet: builds are only offered to the
iPhones Kiwi serves.

Android builds download the original apps from Google Play through an anonymous session
and patch them; the same caution applies. [Android apps](#android-apps) says which
services that involves.

## Turn it on

Open **Settings › Kiwi Builds** in Kiwi and switch it on; a confirmation repeats the note
above. Then open **Sources › Kiwi Builds** (`/builds`) and add the apps you want. A new
Kiwi adds no app by itself.

- While it is off, nothing is scheduled or built, Sources hides the Kiwi Builds row and
  every `/api/automation` endpoint answers `409 builds_off`.
- Turning it off cancels queued and running builds. Releases that were already published
  stay in the Kiwi Builds source and remain installable.
- A Kiwi that had added apps before this setting existed keeps it on.

Each app's page links to its own settings (`/builds/<app>`). Older `/settings/builds` and
`/automation` links still work.

## Requirements

- An Apple Silicon Mac with full Xcode (including the iPhoneOS SDK) and
  [Homebrew](https://brew.sh); macOS supported by the pinned
  [UnfairPlay](https://github.com/DrWhax/unfairplay) revision.
- An Apple account that can download the apps from the App Store, the same one that is
  signed in to the Mac App Store. Kiwi acquires an app's free license if the account does
  not own it yet.
- Passwordless `sudo` for your user while you enroll the Mac: enrollment runs
  `sudo -n unfairplay setup`. Builds themselves do not use sudo.
- Kiwi installed with `./install.sh` (see [install.md](install.md)) and able to sign apps
  (Settings › Signing).

## How a build works

For each app, Kiwi checks Apple for the current build on the app's schedule, downloads the
original app with your account, decrypts it using the Mac's private enrollment, compiles the
tweaks from pinned sources, injects them, and signs the candidate with Kiwi's normal
identity and profiles. Only a validated and signed candidate enters the **Kiwi Builds**
source in Browse; install it with **View App** through the usual iOS confirmation.

This runs inside the Kiwi service: the web app does not need to stay open, no other
service is started, and no artifact is reachable from outside Kiwi. Tailscale access and
Kiwi's normal request protections apply.

## What is built locally

- The original app comes from Apple with your account; nothing comes from a third-party
  AltStore feed. Builds are never published outside Kiwi.
- Tweaks and tools are compiled on the Mac from sources pinned to full commit hashes in
  `automation/recipes.json`. Each pinned source is fetched from GitHub once into
  `~/.kiwi-builder-tools/sources` (setup fetches them all) and builds copy it from there,
  so builds do not contact GitHub. A recipe update fetches only the new pins.
- NeoFreeBird's build script downloads the newest OpenSSL; Kiwi stages a pinned OpenSSL
  (4.1.0) and FFmpeg (n8.1.2) instead and caches the compiled result.
- Not compiled by Kiwi: the `CydiaSubstrate.framework` hooking runtime that cyan bundles and
  injects for Substrate-based tweaks (YouTube, YouTube Music, X), and the prebuilt FFmpeg
  libraries committed in YTMusicUltimate's repository. Both come from the pinned commits.
  NeoFreeBird's prebuilt Safari extension is not included.
- Some optional features talk to third-party services when used: Return YouTube Dislikes
  (`returnyoutubedislikeapi.com`), YTMusicUltimate's SponsorBlock option, and NeoFreeBird's
  Tenor GIF option. TwitchAdBlock only talks to Twitch.

## Apps

| App | Tweak (pinned) | Notes |
|---|---|---|
| YouTube | YouMod with YouPiP, Return YouTube Dislikes, YTABConfig, YouQuality, YouGroupSettings, YTVideoOverlay, UncappedAVPlayer | Optional tweaks are choices |
| YouTube Music | YTMusicUltimate 2.4.1 ([mrdrvt99 fork](https://github.com/mrdrvt99/YTMusicUltimate): upstream plus a download crash fix), optional Return YouTube Music Dislikes, YouMusicPiP, YTMABConfig | Built with `SIDELOADING=1` (Google sign-in and keychain fixes). Sign-in may need a second try |
| Twitch | TwitchAdBlock 0.3.0 ([wkangg fork](https://github.com/wkangg/TwitchAdBlock); upstream is abandoned and crashes on Twitch 24.5+) | Experimental. Sideload fixes are built without theos-jailed (one Makefile line removed). No proxy; no live notifications |
| X | NeoFreeBird 7.3.0 (v7) | Experimental. Built on the X version NeoFreeBird supports (12.38.1), not the newest. Sign in through x.com in the app (X blocks native login for re-signed apps; passkeys don't work). Choose "Twitter" or "X" as the Home Screen name. X's terms forbid modified clients |
| Reddit | RedditFilter 1.3.0 ([and101 fork](https://github.com/and101/RedditFilter)) with RedditSideloadFix | Builds Reddit 2026.43.0. Promoted posts and comment ads are hidden by default. On the email or username login screen, choose **Other options → Log in with browser**, or paste an emailed login link. Google sign-in may fail; use your Reddit username and password. Extra filters are under **Profile → Settings → RedditFilter** |

RedditFilter and its sideload fix are compiled together with `SIDELOADED=1`. Kiwi removes
the Makefile's theos-jailed module selection and injects both compiled dylibs with cyan.
Two login callbacks are assigned to local block variables before `%orig` calls so the
pinned Logos parser can compile them; their behavior is preserved. Ad removal needs no
proxy or Reddit API credentials. Optional keyword, recommendation and other filters are
configured in the app; Direct-play RedGIFs, when enabled there, connects to RedGIFs
directly.

NeoFreeBird is built with `TARGET=iphone:clang:latest:15.0` (its Makefile names an older
SDK whose deployment target current SDKs reject), and its sideload build produces dylibs
rather than a `.deb`; both are in the recipe.

Every app loses push notifications, widgets, share and other app extensions (a wildcard
development profile cannot grant their entitlements, and injection removes extensions).

## Android apps

Kiwi Builds makes patched Android apps on the same Mac and serves them to your Android
phones and tablets as APK downloads. Add them under **Kiwi Builds › Add an Android App**, or
in [Kiwi's Android app](android-app.md): its **Builds** tab lists every app below with an
**Install** button. One tap adds the app with the default patches (or **Customise patches**
first), builds it on demand and installs it once it is ready. The first build takes about 3
to 8 minutes; you can leave the app, and Kiwi notifies you when the build is ready (tap the
notification to install it; Android doesn't let Kiwi open the installer while it is in the
background). Builds started this way go before scheduled checks, and the row shows how many
builds are ahead. Before building, the sheet warns when a copy of the app from elsewhere is
installed: Android can't replace it with Kiwi's build, so uninstall it first (its data on
the phone is lost).

| App | Patches (pinned) | Installs as | Notes |
|---|---|---|---|
| YouTube 21.30.216 | Morphe Patches 2.1.0 | `app.morphe.android.youtube` | Installs beside the original app. Signing in to a Google account needs [MicroG RE](https://github.com/MorpheApp/MicroG-RE); the app links to it the first time it opens |
| YouTube Music 9.30.53 | Morphe Patches 2.1.0 | `app.morphe.android.apps.youtube.music` | As YouTube |
| Reddit 2026.40.0 | Morphe Patches 2.1.0 | `com.reddit.frontpage` | Keeps Reddit's own package name: uninstall the Play Store version first. Its version code is raised to the maximum so that a store never replaces it |

All three need Android 12L or later on a 64-bit ARM device. The patches' options
(SponsorBlock, layout, feed filters and so on) are set inside each app.

The apps keep their own name and icon: the recipes leave out the patch that would replace
them (`disabled_patches` in `automation/android.json`). YouTube and YouTube Music install
beside the original apps under a different package name, so a phone that still has the
original shows two apps with the same name and icon; disable or uninstall the original to
tell them apart.

### What a build does

1. **Fetches the pinned tools**, once: a Java runtime (Eclipse Temurin 21), the Morphe
   patcher, the patch bundle and Google's `apksigner` (Android build-tools). Each is checked
   against the SHA-256 and size in `automation/android.json` before use.
2. **Downloads the original app** at the exact version the recipe pins (below).
3. **Proves who signed it.** Kiwi verifies the APK Signature Scheme v2/v3 signature over
   every byte of each file and compares the signing certificate with the publisher's
   certificate pinned in the recipe (Google's for YouTube and YouTube Music, Reddit's for
   Reddit). The package name, version and version code must match as well. A file that
   fails is discarded and the next source is tried.
4. **Patches it** with the pinned patcher, keeping only the 64-bit ARM code. The patcher
   writes an unsigned app and never sees the signing key; on Linux it runs in a sandbox
   ([How the patcher is contained](#how-the-patcher-is-contained)).
5. **Signs it** with this Kiwi's own key, using `apksigner` (APK Signature Scheme v2 and
   v3), after checking that the patcher's output carries no signature of its own.
6. **Checks the result**: every patch applied, the expected package, version and version
   code, Kiwi's own signature over the whole file, and the alignment Android needs. Only
   then is it published.

### Where the original app comes from

Kiwi tries these in order and takes the first that supplies the pinned version:

1. **A folder on the Mac**, `automation/android/originals/` in the data directory. Put an
   `.apk`, `.apks`, `.xapk` or `.apkm` there yourself and no download is needed.
2. **Google Play**, with an anonymous session. Play serves any published version of a free
   app to a session that asks for its version code. The session comes from the public
   token dispenser of the open-source [Aurora Store](https://auroraoss.com)
   (`auroraoss.com/api/auth`), so no Google account of yours is involved. The dispenser
   receives the generic device profile in `automation/android.json` (a Pixel 9a) and sees
   the Mac's IP address; Google sees the same profile and the download. The session is
   kept for 30 minutes in `automation/android/play-session.json` (mode 0600).
3. **APKPure**, at the same version.

A source is only a transport: whatever it returns is accepted solely because its signature
verifies against the pinned publisher certificate, so a mirror cannot hand Kiwi a modified
app. The verified original is kept in `automation/tools/android/originals/` and reused by
later builds of the same version. When no source has it, the job fails with each source's
reason and Kiwi retries on the next check.

The dispenser limits how many sessions an address may request. A build needs one, and
rebuilds of a cached version need none.

### What is not compiled by Kiwi

The Java runtime, the patcher (`morphe-desktop`, GPL-3.0) and the patch bundle
(`morphe-patches`) are downloaded as their publishers' release files and run as separate
programs; they are not part of Kiwi and are not redistributed with it. MicroG RE, which
YouTube and YouTube Music need for signing in, is installed by you from its own project.
The patched apps talk to the services their patches use (for example SponsorBlock and
Return YouTube Dislike when those are on).

### How the patcher is contained

The patcher and the patch bundle are someone else's code, and a patch bundle is a program:
it runs inside the patcher with everything the patcher can reach. Kiwi therefore treats the
patch step as untrusted and keeps its own secrets out of it:

- **The key stays out.** The patcher writes an unsigned app (Morphe's `--unsigned`). It is
  never given the keystore, its password or a file naming either. Kiwi then checks that the
  output is a plain, unsigned package (no v1, v2 or v3 signature, and not a link) and signs
  it itself with the pinned `apksigner`, which gets the password through its environment.
- **A minimal environment.** The patcher, `apksigner` and `keytool` start with only `PATH`,
  `LANG`, `LC_ALL`, `HOME` and `TMPDIR` (inside the job folder), plus `MORPHE_DATA_DIR` for
  the patcher: nothing of Kiwi's own environment.
- **A sandbox on Linux and in Docker** (`automation/sandbox.py`), made of features any
  unprivileged process may use, so it needs no root, setuid helper or extra container
  privileges:
  - **Landlock** (Linux 6.2 or later): the patcher may read only the Java runtime, its own
    jar, the patch bundle, the original app and the system libraries and settings a Java
    program needs, and write only its own job folder. Kiwi's data directory (the signing
    key, `kiwi.db`, other jobs, artifacts) and everything else is out of reach, including
    truncating or renaming files. On Linux 6.7 and later Landlock also refuses TCP
    connections, and on 6.12 and later signals to processes outside the sandbox.
  - **A system call filter** (seccomp) that refuses creating any socket, so the patcher has
    no network at all: no TCP or UDP, no Unix sockets such as tailscaled's, and no
    connection to Kiwi itself. It also refuses io_uring, bpf, perf events, userfaultfd and
    the kernel keyrings.
  - `no_new_privs`, no core dumps, a file size limit (8 GB) and a CPU time limit (3 hours)
    besides the hour the job may take. A process the patcher leaves running is ended when
    it exits. (A limit on the number of processes is not used: Linux counts that per user,
    for all of Kiwi.)

  `apksigner` runs in the same sandbox, with the keystore added to what it may read and
  the job folder to what it may write.
- **macOS has no such sandbox.** On a Mac the patcher runs as the Kiwi user without a
  sandbox, with the key kept out and the minimal environment as above. The build log says
  so.

Each job's private log (`automation/jobs/<id>/build.log`) records what the sandbox could
apply, for example `kiwi: sandbox: Landlock ABI 6 (...; no TCP), seccomp (no sockets)`, or
`kiwi: sandbox unavailable: <reason>`. The pinned official patches build even where the
sandbox is unavailable (an older kernel, a container whose seccomp profile blocks
Landlock); a recipe can instead require it (`"sandbox": "required"`), and then the build
stops with a message saying why the computer can't provide one. `python3
automation/sandbox.py --probe` prints what a computer supports. Docker's default seccomp
profile allows Landlock: in Kiwi's image the sandbox works without any change to
`compose.yaml`.

### The signing key

The first Android build creates a 4096-bit RSA key in
`automation/android/keystore.p12`, with its password beside it in `keystore.pass` (both
mode 0600). It never leaves the computer Kiwi runs on and no API returns it. Android installs an update over
an app only when both carry the same signature, so **back these two files up** with the
rest of the data directory: without them, every Android app Kiwi built has to be
uninstalled before a build signed with a new key can be installed. Kiwi never makes a new
key silently: when `keystore.p12` is missing, or isn't the key in `signer.json`, Android
builds stop with a message to restore both files from a backup. To start over with a new
key on purpose, delete `automation/android/signer.json`. Settings › **Android
Signing Key** on an Android device shows the SHA-256 of its certificate, the same
fingerprint Android shows for apps Kiwi signed; the web app never offers the key itself. The same key signs
[Kiwi's own Android app](android-app.md), which Kiwi offers to Android phones while Kiwi
Builds is on.

The password never appears on a command line, where other local users could read it while
a build runs: `keytool` and `apksigner` get it through their environment. The patcher never
gets the key or its password at all; Kiwi signs its output afterwards
([How the patcher is contained](#how-the-patcher-is-contained)). Patched apps carry APK
Signature Scheme v2 and v3 signatures with this key (earlier Kiwi versions signed them with
v2 only); the certificate is the same, so Android installs the new builds over the old.

### Install on Android

Install [Tailscale](https://tailscale.com/download/android) on the device, sign in as the
owner and open Kiwi's address in the browser. Kiwi recognises Android devices and asks once
to confirm ([First use](using-kiwi.md#first-use)): Browse then shows Android apps only, and
the **Builds** tab, where you add them, takes the place of Sources (whose AltStore sources
list iPhone apps). Settings shows the fingerprint of this Kiwi's Android signing key under
**Android Signing Key**; back up the key itself from the data directory
([The signing key](#the-signing-key)).

1. Open the app in Kiwi and tap **Install**. The browser downloads the APK.
2. Open the download. The first time, Android asks you to allow that browser to install
   apps; Play Protect may ask to scan an app it doesn't know.
3. For an update, tap **Update** and open the new download. Android installs it over the
   old build and keeps your data.

Kiwi can't see which apps an Android device has installed. It records an install when the
download completes, so My Apps shows what you downloaded; use **Reinstall** on the app's
page for a fresh copy, and **Forget** when you removed the app. Android apps don't expire,
and Kiwi's signing warnings don't apply to them.

### Requirements and limits

- Python 3.13 on the Mac (`brew install python@3.13`); on Linux Python 3.12 or newer
  (`python3`, or the one `--python`/`KIWI_PYTHON` names, installed outside home directories
  for `install.sh`'s service; the Docker image has 3.13). An older Python fails the build
  with a message saying so. Nothing else: the setup below, the Apple account, Xcode and
  enrollment are for iPhone apps only.
- Linux on x86-64 or ARM64, or a Mac: the pinned Java runtime exists for those. The
  patcher's sandbox needs Linux 6.2 or later (6.7 or later adds Landlock's own TCP block);
  on older kernels and on a Mac the official recipes build without it.
- About 1 GB of disk for the tools and cached originals, plus 75 to 180 MB for each
  retained release (the newest 5 per app), and network access to GitHub, `auroraoss.com`,
  Google Play and APKPure.
- Each recipe pins one app version, the one the pinned patches support. A newer version
  follows with a Kiwi update that moves the pins; a Kiwi update that changes the recipe,
  the patches or the runner rebuilds the app once.
- Builds are inspected, not run: package, version, signature and applied patches are
  checked, playback and sign-in are not. The apps above were built and inspected on Linux
  and have not been installed on a physical Android device yet.

## Configure in Kiwi

- **Kiwi Builds** lists the added apps with their status, the apps that can be added, and
  the shared Mac setup. Adding an app shows its limitations and options first; **Add**
  saves the options and starts the first build.
- Per app: build updates automatically or not, the check interval, optional tweaks, and the
  Home Screen name where there is a choice. **Save Changes** persists them; **Build Changes**
  applies new choices now, **Check Now** looks for an update, **Rebuild** produces a fresh
  revision of an unchanged recipe, and a queued or running job can be cancelled.
- A job keeps the settings it started with; its history records them.
- **Remove from Kiwi Builds** stops an app's builds and takes it out of the source. Installed
  copies keep working until they expire. Adding it again continues its revision numbers.
- The **Kiwi Builds** source is built in. Its switch controls source visibility only; it
  cannot be removed through the source-delete API.

Apple sign-in, enrollment and toolchain installation stay on the Mac; the setup status in
Kiwi Builds says which step is missing.

## Set up the Mac

Kiwi writes `runner.py` and `recipes.json` into its data directory every time it starts, so
`./install.sh` puts them in place; you never copy them by hand. Run these in Terminal on the
Mac, logged in to the desktop as the user Kiwi runs as:

```sh
brew install python@3.13   # once, if Homebrew's Python 3.13 is missing
KIWI_DATA="$HOME/Library/Application Support/Kiwi"
/opt/homebrew/bin/python3.13 "$KIWI_DATA/automation/runner.py" setup --data-dir "$KIWI_DATA"
```

Setup installs Go, GNU make, ldid and dpkg with Homebrew (missing ones only; it never
upgrades installed tools), builds the pinned ipatool, UnfairPlay and cyan, installs pinned
Theos, Logos and headers (YouTubeHeader, PSHeader, YouTubeMusicHeader), then fetches every
app's pinned sources. It uses Xcode's local SDK. The ipatool adapter adds the library's
existing `LatestExternalVersionID` field to its JSON output; it does not change Apple's
version selection. Setup is safe to repeat, and must be repeated after a Kiwi update that
changes the toolchain (Kiwi Builds then reports "Tweak toolchain" as not ready).

The toolchain and build workspaces live in `~/.kiwi-builder-tools`, because Theos cannot
use paths with spaces; `automation/tools` in the data directory links there. An existing
`automation/tools` link (for example to `~/Code/Kiwi-builder-tools`) is kept.

Sign in to the App Store downloader, separately from the Mac App Store:

```sh
"$HOME/.kiwi-builder-tools/bin/ipatool" auth login
```

Use the Apple account that is signed in to the Mac App Store. ipatool keeps its own login
session; the Mac App Store's sign-in alone does not authenticate it. Apple may ask for a
two-factor code, and later for another sign-in. Credentials stay in ipatool's local
keyring; they are never sent through Kiwi's web app or stored in Kiwi's database.

Install any iPhone or iPad app from the Mac App Store and open it once; UnfairPlay uses its
encrypted binary and Apple's license for it as a sample. Then enroll the Mac with it, which
creates the reusable private decryption model, and check the result:

```sh
/opt/homebrew/bin/python3.13 "$KIWI_DATA/automation/runner.py" enroll \
  --data-dir "$KIWI_DATA" --app "/Applications/<App>.app"
/opt/homebrew/bin/python3.13 "$KIWI_DATA/automation/runner.py" doctor --data-dir "$KIWI_DATA"
```

Enrollment packages the encrypted app and license Apple already installed and runs
UnfairPlay's setup with `sudo -n`, so passwordless sudo must work for your user at that
moment. It does not need the ipatool sign-in, and it validates the model by decrypting the
sample. The model stays in the data directory with mode 0600. A macOS or Apple account
change may require enrolling again. When `doctor` reports every check ready, use
**Check Now** in Kiwi Builds.

## Build behavior

- Preferences control the 15–1440 minute check interval, optional tweaks and the app name.
  YouTube checks every hour by default, the other apps every 6 hours.
- Each recipe identifies the exact Apple external build, source commits, runner digest,
  toolchain, the app's recipe and options. Unchanged successful recipes are skipped. A
  Kiwi update that changes the runner or recipes therefore rebuilds each app once.
- Both `CFBundleVersion` and `CFBundleShortVersionString` stay exactly as Apple supplied
  them: YouTube uses the former as its API client version (changing it leaves its feeds
  empty), and the other apps' version checks are kept intact the same way. A separate
  per-app monotonic `revision` (also stamped as `KiwiBuildRevision` in the main plist) makes
  changed patches on the same version visible as updates. Kiwi shows `21.43.5 · Kiwi 7`, and
  its history selector includes it.
- For X and Reddit, Kiwi builds the App Store version the pinned tweak release targets. It
  finds Apple's build of that version among the newest 40 and remembers the lookups.
- The builder checks the bundle id and version, archive paths, all Mach-O executable slices
  for remaining encryption (except extensions, which injection removes), and that the main
  executable loads each app's marker libraries (YouMod; YTMusicUltimate; TwitchAdBlock;
  BHTwitter and NeoFreeBird's zxPluginsInject sideload fix; RedditFilter and RedditSideloadFix).
  Kiwi then verifies metadata, hashes, signing and provisioning through its normal build
  path before publication.
- There is one build worker. Jobs of other apps wait as `queued` ("Waiting for YouTube").
  Cancel terminates the builder's process groups and prevents publication. A signing task
  already started can finish privately. A restart marks interrupted and queued jobs failed;
  the next scheduled check retries them.
- A failure keeps the current release. Three failed attempts for the same recipe stop
  automatic retries; **Rebuild** retries it explicitly. Check history keeps the last 30 jobs
  per app; the source keeps the last 10 releases (Twitch 5, X 3).
- These checks do not verify playback, tweak behavior or compatibility on a physical phone.
  Try a new release before removing a working version. New app releases can need source
  compatibility fixes; automatic detection cannot repair arbitrary upstream changes.

## Files and maintenance

Paths under `automation/` are in the data directory, `~/Library/Application Support/Kiwi`.

| Location | Purpose |
|---|---|
| `kiwi.db` meta `automation_v2` | Per-app preferences, revisions and health, and job history. Migrated once from `youtube_automation_v1`, which is left in place for a rollback |
| `automation/runner.py`, `automation/recipes.json` | The runner and pinned recipes built into Kiwi, rewritten at every start |
| `automation/android.py`, `automation/apk.py`, `automation/sandbox.py`, `automation/android.json` | The Android runner, its APK signature verifier, the sandbox for the patcher and the pinned Android recipes, rewritten at every start |
| `automation/kiwi-app.json`, `automation/kiwi-app.log` | The pinned release of [Kiwi's Android app](android-app.md), and the private log of signing it |
| `automation/android/keystore.p12`, `keystore.pass` | This Kiwi's Android signing key and its password, mode 0600. Back them up |
| `automation/android/originals/` | Original Android apps you supply by hand (optional) |
| `automation/android/play-session.json` | The anonymous Google Play session, mode 0600 |
| `automation/tools/android/` | The pinned Java runtime, patcher, patch bundle and `apksigner`, and verified originals |
| `~/.kiwi-builder-tools` | Space-free private toolchain and transient build workspaces; `automation/tools` links here |
| `~/.kiwi-builder-tools/sources/<owner>__<repo>/<commit>` | Pinned source cache (a `.complete` file marks a finished fetch) |
| `~/.kiwi-builder-tools/cache/ffmpeg-kit/<key>` | NeoFreeBird's compiled FFmpeg stack, keyed by its inputs and the SDK |
| `~/.kiwi-builder-tools/versions/<app id>.json` | Remembered versions of Apple build ids (for the pinned X and Reddit versions) |
| `automation/models/*.bin` | Private account-specific decryption model, mode 0600 |
| `automation/jobs/<id>/build.log`, `automation/setup.log`, `automation/enrollment.log` | Private diagnostics; raw tool output is never served over HTTP |
| `automation/artifacts/<sha>.ipa`, `<sha>.json` | Retained patched input and build provenance, addressed by SHA-256 |
| `automation/artifacts/<sha>.apk`, `<sha>.json` | A patched, signed Android app as it is served, and its provenance (source of the original, its digest, the applied patches) |

Original account-personalized downloads and decrypted intermediates are removed after each
build, including after a failure or cancellation. Do not publish models, originals or
private logs. Kiwi's ordinary signed IPA and install-token lifecycle handles installation.
Back up the private models along with `kiwi.db` if avoiding re-enrollment matters.

Hourly cleanup removes unreferenced artifacts after seven days, stale workspaces after one
day, and job log directories after 30 days. Inputs referenced by a retained source, signing
build or installed app are kept. Cleanup waits while a build is queued or running.

`./uninstall.sh` removes Kiwi and, with `--purge`, its data directory including the models.
It leaves `~/.kiwi-builder-tools` and the Homebrew packages setup installed; delete them
yourself if you no longer need them.

## Update or add a recipe

To update a tweak or the build tooling, review and change the full commit hashes in
`automation/recipes.json` (and the tweak's version in `tweak`; for X and Reddit also
`app_version`), compile it (below), and install Kiwi again with `./install.sh`. Run setup
again when the toolchain changed. Existing releases stay usable. The runner and recipes are
built into the Kiwi binary, so editing the installed copies alone does not last.

For Android, change `automation/android.json`: the patcher and patch bundle (URL, SHA-256,
size), and per app the `app_version` and `version_code` the patches support, the
publisher's `certificate` (SHA-256 of the signing certificate) and the package the patched
app carries (`bundle_id`, and `patched_version_code` when a patch changes the version
code). `python3 automation/apk.py <file.apk>` prints a package's verified certificate,
version and version code. The Android runner works on Linux as well as macOS, so a changed
recipe can be built anywhere before Kiwi is installed again.

To add another app, add a recipe: App Store id and bundle id, packages with their pinned
repositories and build flags (`make`, `submodules`, `siblings`, `patches`, `target` and
`products` for tweaks whose sideload build is not a `.deb`), marker libraries, catalogue
text and limitations. Compile it on the Mac first:

```sh
# Compile an app's tweaks (all optional ones) without downloading or decrypting the app.
/opt/homebrew/bin/python3.13 "$KIWI_DATA/automation/runner.py" compile \
  --data-dir "$KIWI_DATA" --recipe youtube-music --job-id 000000000000000000000001
# Repository checks (any machine; native compile and signing checks run on the Mac).
python3 -m unittest discover -s automation -v
cargo test --workspace
cd web && npm test && npm run build
```

## Testing status

Every recipe above has been compiled, built, signed and inspected on a Mac (macOS 27.0.1,
Xcode 27.1, iPhoneOS 27.1 SDK): team-suffixed bundle id, both Apple version fields
unchanged, `KiwiBuildRevision` set, the injected libraries in the main executable's load
commands with their dependencies present, `cryptid 0` in every slice, no extensions,
minimal entitlements and `codesign --verify --deep --strict` passing. YouTube, YouTube
Music, Twitch and X were installed and used on an iPhone. Reddit's login, browsing and ad
removal have not been tested on a physical iPhone yet. Each new release still needs its own
check on the phone.

## Troubleshooting

- **Setup needed** in Kiwi Builds: run `doctor` (above); it names the missing step (build
  tools, tweak toolchain, App Store sign-in or enrollment).
- **App Store sign-in** not ready: run `ipatool auth login` again; Apple's sessions expire.
- **Enrollment fails:** passwordless sudo must work (`sudo -n true`), the sample app must
  have been opened once, and the macOS version must be supported by the pinned UnfairPlay.
  Details are in `automation/enrollment.log`.
- **A build fails:** its log is `automation/jobs/<id>/build.log`. The current release stays
  available; fix the cause and use **Rebuild**.
- **"No source could supply the original …"** (Android): the message lists each source's
  reason. A rate-limited anonymous sign-in clears by itself; otherwise download the named
  version yourself and put it in `automation/android/originals/`, then use **Rebuild**.
- **Android says "App not installed"**: an app with the same package signed by someone
  else is installed (the Play Store's Reddit, or a build from another patcher). Uninstall
  it first.

More in [troubleshooting.md](troubleshooting.md). The HTTP endpoints are described in
[api.md](api.md#kiwi-builds-automation).
