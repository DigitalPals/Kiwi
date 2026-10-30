# Kiwi Builds

Kiwi Builds patches App Store apps with open-source tweaks and signs them on your Mac:
**YouTube** (YouMod), **YouTube Music** (YTMusicUltimate), **Twitch** (TwitchAdBlock),
**X** (NeoFreeBird) and **Reddit** (RedditFilter). Two experimental apps bring Rambler
dictation, Google's speech recognition that drops filler words and applies spoken
corrections, to the iPhone: **Gboard Rambler**, a patched Gboard keyboard, and **Rambler
PoC**, a small dictation app Kiwi compiles from source. Signed releases appear in the
built-in **Kiwi Builds** source and install and update like any other app in Kiwi.

It is optional and off by default. Kiwi itself works without it; see the
[README](../README.md) and [installation guide](install.md) first.

## Before you turn it on

Kiwi Builds downloads App Store apps with your own Apple account, decrypts them on your
Mac with [UnfairPlay](https://github.com/DrWhax/unfairplay) and injects open-source
tweaks into them. Modified apps may violate the apps' terms of service; X's terms forbid
modified clients outright. Use it only for apps you may use, on your own devices, at your
own risk. Kiwi publishes nothing outside your tailnet: builds are only offered to the
iPhones Kiwi serves.

The two Rambler apps send your audio to a private, undocumented Google speech service,
using the caller metadata Google ships in its Android Gboard app. Google may block that at
any time, and it may be against Google's terms.

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

Rambler PoC needs only an Apple Silicon Mac with full Xcode, Python 3.13 and Kiwi able to
sign apps; no App Store account, enrollment or tweak toolchain. The App Store apps,
Gboard Rambler included, need:

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
- For Gboard Rambler, profiles for its app and keyboard with Kiwi's App Groups (see
  below).

## How a build works

For each App Store app, Kiwi checks Apple for the current build on the app's schedule, downloads the
original app with your account, decrypts it using the Mac's private enrollment, compiles the
tweaks from pinned sources, injects them, and signs the candidate with Kiwi's normal
identity and profiles. Only a validated and signed candidate enters the **Kiwi Builds**
source in Browse; install it with **View App** through the usual iOS confirmation.

**Rambler PoC** is compiled from the Swift sources built into Kiwi (see
[Gboard Rambler](../experiments/gboard-rambler/README.md)). Its first build downloads a
SHA-256-pinned PixelBoard APK only to read Google's caller metadata from it; no Android
code is run or included, and the cached APK is checked on every use. Each build runs the
protocol tests and a live speech correction through the macOS client before it compiles
the iPhone app, which is then validated and signed as usual. A failure keeps the last
release.

**Gboard Rambler** puts the same client into Gboard 2.3.19 from the App Store. Unlike the
other apps it keeps its keyboard extension, which is decrypted, patched and signed too.
Both the app's and the keyboard's profiles must grant
`group.com.kiwi.gboard.<TEAM>.keyboard` and `group.com.kiwi.gboard.<TEAM>.common`: register
the team-suffixed app and keyboard ids with those App Groups in Xcode or the developer
portal, then rescan in **Settings › Signing**. Signing fails rather than drop the groups.

To use it, open Gboard Rambler once to allow the microphone, add its keyboard in iOS
**Settings › General › Keyboard › Keyboards** with **Allow Full Access**, and tap Gboard's
microphone. Pause or tap **Stop** to get the corrected text. iOS doesn't let keyboards use
the microphone, so Gboard opens to record; when iOS offers no way back, a **Swipe back to
your app** screen says so. **Rambler** in the Gboard app has the recognition options,
**Copy text** for the last result, **Dictate in keyboard** (off by default: records in the
background so you can speak in your own app) and **Diagnostics**. There are no live
partial results and no offline recognition.

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
| Rambler PoC | Native Swift client 0.1.0 | Experimental. A standalone dictation app: record, then copy the corrected text. Needs internet; audio goes to Google. Not tried on an iPhone yet |
| Gboard Rambler | Native Swift/Objective-C client 0.1.8 | Experimental. Gboard 2.3.19 with Rambler dictation. Needs the App Group profiles above; audio goes to Google. Recording and insertion work on an iPhone; the swipe-back screen has only been tested in the simulator |

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

The other patched App Store apps lose push notifications, widgets, share and other app extensions (a wildcard
development profile cannot grant their entitlements, and injection removes extensions).
Rambler PoC has no extensions; Gboard Rambler keeps its keyboard and needs the App Group
profiles above. Google may change or close its private speech service at any time.

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

Kiwi writes `runner.py`, `recipes.json` and the native source manifest into its data
directory every time it starts, so `./install.sh` puts them in place; you never copy
them by hand.

For Rambler alone, full Xcode, Python and configured Kiwi signing are sufficient. The
following downloader/toolchain/enrollment steps apply to patched App Store recipes.
Run them in Terminal on the Mac, logged in to the desktop as the user Kiwi runs as:

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
  YouTube checks every hour by default, Rambler and Gboard daily, the other apps every 6 hours.
- Each recipe identifies the exact Apple external build, source commits, runner digest,
  toolchain, the app's recipe and options. Unchanged successful recipes are skipped. A
  Kiwi update that changes the runner or recipes therefore rebuilds each app once.
  Native recipes use their source digest and pinned version instead of Apple's build id.
- Both `CFBundleVersion` and `CFBundleShortVersionString` stay exactly as Apple supplied
  them: YouTube uses the former as its API client version (changing it leaves its feeds
  empty), and the other apps' version checks are kept intact the same way. A separate
  per-app monotonic `revision` (also stamped as `KiwiBuildRevision` in the main plist) makes
  changed patches on the same version visible as updates. Kiwi shows `21.43.5 · Kiwi 7`, and
  its history selector includes it.
- For X, Reddit and Gboard, Kiwi builds the App Store version the pinned tweak release targets. It
  finds Apple's build of that version among the newest 40 and remembers the lookups.
- The builder checks the bundle id and version, archive paths, all Mach-O executable slices
  for remaining encryption (except extensions which the other recipes remove), and that the main
  executable loads each app's marker libraries (YouMod; YTMusicUltimate; TwitchAdBlock;
  BHTwitter and NeoFreeBird's zxPluginsInject sideload fix; RedditFilter and RedditSideloadFix).
  Kiwi then verifies metadata, hashes, signing and provisioning through its normal build
  path before publication. Gboard additionally pins its inspected image hashes and requires
  its keyboard executable to load the native patch.
- There is one build worker. Jobs of other apps wait as `queued` ("Waiting for YouTube").
  Cancel terminates the builder's process groups and prevents publication. A signing task
  already started can finish privately. A restart marks interrupted and queued jobs failed;
  the next scheduled check retries them.
- A failure keeps the current release. Three failed attempts for the same recipe stop
  automatic retries; **Rebuild** retries it explicitly. Check history keeps the last 30 jobs
  per app; the source keeps the last 10 releases (Twitch 5, X, Rambler and Gboard 3).
- These checks do not verify playback, tweak behavior or compatibility on a physical phone.
  Try a new release before removing a working version. New app releases can need source
  compatibility fixes; automatic detection cannot repair arbitrary upstream changes.

## Files and maintenance

Paths under `automation/` are in the data directory, `~/Library/Application Support/Kiwi`.

| Location | Purpose |
|---|---|
| `kiwi.db` meta `automation_v2` | Per-app preferences, revisions and health, and job history. Migrated once from `youtube_automation_v1`, which is left in place for a rollback |
| `automation/runner.py`, `automation/recipes.json` | The runner and pinned recipes built into Kiwi, rewritten at every start |
| `automation/native/rambler.json` | Native Swift/build/test sources embedded in Kiwi; contains no caller credentials |
| `automation/native/gboard-rambler.json` | Shared speech client plus native Gboard hooks and the hash-pinned packager |
| `~/.kiwi-builder-tools` | Space-free private toolchain and transient build workspaces; `automation/tools` links here |
| `~/.kiwi-builder-tools/sources/<owner>__<repo>/<commit>` | Pinned source cache (a `.complete` file marks a finished fetch) |
| `~/.kiwi-builder-tools/cache/ffmpeg-kit/<key>` | NeoFreeBird's compiled FFmpeg stack, keyed by its inputs and the SDK |
| `~/.kiwi-builder-tools/versions/<app id>.json` | Remembered versions of Apple build ids (for the pinned X and Reddit versions) |
| `~/.kiwi-builder-tools/native-inputs/<sha>.apk` | Private pinned Rambler metadata input; checksum verified before extraction |
| `automation/models/*.bin` | Private account-specific decryption model, mode 0600 |
| `automation/jobs/<id>/build.log`, `automation/setup.log`, `automation/enrollment.log` | Private diagnostics; raw tool output is never served over HTTP |
| `automation/artifacts/<sha>.ipa`, `<sha>.json` | Retained patched input and build provenance, addressed by SHA-256 |

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
commands with their dependencies present, `cryptid 0` in every slice, extensions
removed except Gboard's required keyboard,
minimal entitlements and `codesign --verify --deep --strict` passing. YouTube, YouTube
Music, Twitch and X were installed and used on an iPhone. Reddit's login, browsing and ad
removal have not been tested on a physical iPhone yet. Each new release still needs its own
check on the phone.

Gboard Rambler's client also passes a simulator harness with 88 checks, including live
spoken corrections, insertion, voice-panel clean-up, cancellation, App Group remapping and
URL routing. The harness uses stand-in classes because Google's device-only Gboard binary
doesn't run in the simulator; the microphone and keyboard hand-off need an iPhone.

## Troubleshooting

- **Setup needed** in Kiwi Builds: run `doctor` (above); it names the missing step (build
  tools, tweak toolchain, App Store sign-in or enrollment).
- **App Store sign-in** not ready: run `ipatool auth login` again; Apple's sessions expire.
- **Enrollment fails:** passwordless sudo must work (`sudo -n true`), the sample app must
  have been opened once, and the macOS version must be supported by the pinned UnfairPlay.
  Details are in `automation/enrollment.log`.
- **A build fails:** its log is `automation/jobs/<id>/build.log`. The current release stays
  available; fix the cause and use **Rebuild**.

More in [troubleshooting.md](troubleshooting.md). The HTTP endpoints are described in
[api.md](api.md#kiwi-builds-automation).
