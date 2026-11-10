# Developing Kiwi

Kiwi is a Rust workspace (the server and its libraries), a Preact web app in `web/` and a
Python build runner for Kiwi Builds in `automation/`. How the parts fit together is in
[architecture.md](architecture.md); the HTTP contract between server and web app is
[api.md](api.md).

Everything builds and its platform-neutral tests run on Linux as well as macOS. Signing,
devicectl and Kiwi Builds' iOS apps need a Mac.

## Requirements

- Rust stable (see `rust-toolchain.toml`; rustup installs it with `clippy` and `rustfmt`)
- Node.js 20 or newer with npm (CI uses Node 24)
- Python 3.13 with pytest, for the build runner's tests
- shellcheck, for the scripts

## Build and test

```sh
# Rust: format, lint, test
cargo fmt --all --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace --locked

# Web app: type check, test, build to web/dist
cd web
npm ci
npm run typecheck
npm test
npm run build
cd ..

# Kiwi Builds runners
python3 -m pytest automation/test_runner.py automation/test_android.py

# Scripts
shellcheck install.sh uninstall.sh deploy/deploy.sh android/scripts/build-in-docker.sh

# Android app (JDK 17+ and an Android SDK; without them, android/scripts/build-in-docker.sh)
cd android
./gradlew test assembleRelease
cd ..
```

The same checks run in CI (`.github/workflows/ci.yml`). Keep them passing.

### Tests that need a Mac

A few tests are `#[ignore]`d because they need real Apple tooling; run them on a Mac by hand:

```sh
# devicectl with booted simulators, and Tailscale (Homebrew's tailscaled, an iPhone on the tailnet)
KIWI_LIVE_IPHONE=<the iPhone's Tailscale IP> cargo test -p kiwi-device --test live_mac -- --ignored --nocapture

# Re-sign a real IPA with your identity and profiles
KIWI_SIGN_IPA=~/app.ipa KIWI_SIGN_OUT=~/out KIWI_SIGN_IDENTITY=<sha1> \
KIWI_SIGN_BUNDLE_ID=com.example.app.<TEAMID> \
  cargo test -p kiwi-signer --test real_sign -- --ignored --nocapture
```

One more needs internet access (it downloads the pinned Java runtime and Android build-tools)
and an unsigned release APK of Kiwi's Android app; it runs anywhere. It signs the APK with
the real Android runner through the server's own job and checks what Kiwi then serves:

```sh
android/scripts/build-in-docker.sh assembleRelease
KIWI_TEST_UNSIGNED_APK=$PWD/android/app/build/outputs/apk/release/app-release-unsigned.apk \
  cargo test -p kiwi-server --test automation -- --ignored --nocapture real_kiwi_app
```

`live_mac` only compiles on macOS. Everything else uses fixtures (recorded `devicectl` and
Tailscale JSON in `crates/kiwi-device/tests/fixtures`, AltStore feeds in
`crates/kiwi-source/tests/fixtures`) and fakes (signer, devices, push, App Store and App Store
Connect servers in `crates/kiwi-server/tests/common`).

## The web app without a server

The web app has a mock API with fixtures for every screen, so you can work on it without a
Mac or a running Kiwi:

```sh
cd web
VITE_MOCK=1 npm run dev
```

Open the printed address.

The mock API follows [api.md](api.md) closely enough to exercise every state (downloads,
signing progress, failures, updates, expiring apps, Kiwi Builds jobs). It cannot build or
install anything.

The README's screenshots (`docs/screenshots/`) are taken from it. To take them again after
changing the interface, keep the demo running and, with Google Chrome installed (or
`CHROME=/path/to/chromium`):

```sh
cd scripts/screenshots && npm install && npm run shoot   # KIWI_DEMO=http://localhost:5173 by default
```

It saves every screen in light and dark at iPhone size and renders `hero.png` from
`hero.html`.

Kiwi Builds jobs in the demo stop at the Mac's setup. Start it with
`KIWI_MOCK_BUILDS=simulate VITE_MOCK=1 npm run dev` and builds run through the Mac's stages
(about 20 seconds each) and publish signed releases in the Kiwi Builds source, ready to install
and update. For a new version to build, release one in the demo's App Store with
`POST /__mock/app-store` and `{"bundle_id": "com.google.ios.youtube", "version": "21.45.0"}`.

The demo answers as an iPhone. Add `KIWI_MOCK_DEVICE=android` and it answers as an Android
phone: Browse lists Android apps only, and a simulated Android build installs as an APK
download. `KIWI_MOCK_DEVICE=desktop` answers as a desktop browser (no platform detected).
`KIWI_MOCK_ONBOARDED=0` starts before the first-run setup, `KIWI_MOCK_HOST=linux` answers as
a Kiwi on Linux (Kiwi Builds' checks are only the missing Mac, and adding or checking an iOS
app answers `409 needs_mac`), `KIWI_MOCK_ANDROID_APP=0` without Kiwi's Android app,
`KIWI_MOCK_ANDROID_APP=failed` as a Kiwi that couldn't sign it (Android devices get the
`android_app` warning), and `KIWI_MOCK_FOREIGN=app.morphe.android.youtube` with that package
installed under another signature.

The demo imitates a Kiwi on a Mac, which signs with the keychain. Add
`KIWI_MOCK_SIGNING=files` and it imitates one on Linux (`Signing.backend: "files"`): the
identity and profiles are uploads, which the `/api/signing/identity` and
`/api/signing/profiles` endpoints add and remove (any base64 is accepted; the password
`wrong` is refused). Without an identity or a usable wildcard profile, `/api/me` warns
about it as the server does (`no_identity`, `no_profile`).

## Running the server locally

```sh
cd web && npm ci && npm run build && cd ..
cargo run -p kiwi-server -- \
  --listen 127.0.0.1:18742 --data-dir /tmp/kiwi-dev --web-dir web/dist \
  --public-url http://localhost:5173
```

On Linux, Kiwi signs with an uploaded identity and profile ([signing-on-linux.md](signing-on-linux.md);
until then signing reports that none was uploaded) and Tailscale lookups fail, but sources,
the catalogue, settings and the API work. Every flag is also an environment variable
(`KIWI_LISTEN`, `KIWI_DATA_DIR`, `KIWI_PUBLIC_URL`, ...; see `kiwi --help`).

To try the container image: `docker build -t kiwi .`, and `docker compose config` checks
`compose.yaml`. CI builds the image on every push without publishing it.

Requests must name the public URL's host or the listen address (see "Security" in
[architecture.md](architecture.md)), so when the Vite dev server proxies to a real Kiwi, pass
the dev server's address as `--public-url` and point Vite at Kiwi:

```sh
cd web
KIWI_API=http://127.0.0.1:18742 npm run dev   # proxies /api and /ota
```

## Trying a change on a Mac

On the Mac itself, `./install.sh` builds and installs your checkout (`./install.sh --check`
first shows what it would change). From another machine, `deploy/deploy.sh` copies the
checkout to the Mac over SSH and runs `install.sh` there:

```sh
KIWI_MAC=you@your-mac deploy/deploy.sh             # or put KIWI_MAC=... in deploy/deploy.env
KIWI_MAC=you@your-mac deploy/deploy.sh --no-build  # reinstall the last build
```

It needs SSH key login, a desktop session of that user on the Mac, and passwordless `sudo`
there (log rotation and `tailscale serve`). The copy lives in `~/kiwi-build` on the Mac, with
its own `target/` and `node_modules/`. Installs keep a backup of `kiwi.db` and roll back to the
previous build when the new one doesn't come up; see [operations.md](operations.md).

## Layout

```
crates/kiwi-source     AltStore sources, releases, IPA download and inspection
crates/kiwi-signer     identities, provisioning profiles, re-signing (macOS)
crates/kiwi-device     devicectl, Tailscale (LocalAPI or CLI), user agents
crates/kiwi-push       VAPID and Web Push
crates/kiwi-server     the `kiwi` binary: HTTP API, SQLite, jobs, scheduler
web/                   the PWA (src/), its mock API (mock/) and tests
automation/            Kiwi Builds runner and pinned recipes
deploy/                LaunchAgent, log rotation and systemd templates, deploy.sh
install.sh             installer for a Mac or Linux; uninstall.sh removes Kiwi
Dockerfile             the container image; compose.yaml runs it with a Tailscale sidecar
```

## Conventions

- Keep the server and `web/src/api/types.ts` in step with [api.md](api.md); the mock API too.
- Messages in API errors are shown to people as they are: write them as sentences.
- Fixtures and tests use example data only: `example.com` logins, `*.example.ts.net` names,
  team `A1B2C3D4E5`, made-up UDIDs. Never commit real device ids, tailnets or certificates.
- Dependencies stay pinned (`Cargo.lock`, `web/package-lock.json`, Kiwi Builds
  recipes pinned to full commit hashes).
