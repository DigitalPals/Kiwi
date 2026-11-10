# Kiwi's Android app

Kiwi has a native Android app ([android/](../android/README.md)): it installs Kiwi Builds'
Android apps, tells Kiwi what is installed and updates them in the background. This page is
about how the app reaches the phones: who builds it, who signs it, and how a new version is
released.

## How Kiwi delivers it

The project publishes the app **unsigned**. There is no project signing key: every Kiwi
signs the app itself, with the same Android key that signs its [Kiwi Builds](kiwi-builds.md#the-signing-key)
apps, and serves it to the owner's phones.

1. A GitHub release workflow builds the unsigned release APK and attaches it, with its
   SHA-256, to a release named after the version.
2. Kiwi pins one release in [`automation/kiwi-app.json`](../automation/kiwi-app.json): its
   URL, SHA-256 and size. The pin ships inside Kiwi, so a Kiwi update brings a new app
   version.
3. Kiwi downloads the pinned APK, checks its size and SHA-256 and that it is an unsigned
   build of `io.github.digitalpals.kiwi` with the pinned version, signs it with its Android
   key and offers it as `/android/kiwi.apk`.

Android installs an update only over an app signed with the same key. A Kiwi signs every
version of the app with its one key, exactly as it signs its builds, so each version
updates the one before. A phone that switches to another Kiwi (another key) has to
uninstall the app first.

Kiwi does this while **Kiwi Builds is on** and Kiwi has a reason to: a device it knows as
Android, or an Android app in Kiwi Builds. It checks at startup and every minute whether
what it serves is still made from the current input (the pinned release, or your own
APKs, see below) and signed with its current key. When it isn't, the Android runner's
`kiwi-app` command runs on Kiwi Builds' worker, one job at a time with the builds:

- it fetches the pinned Java runtime and Android SDK Build-Tools once (only
  `apksigner.jar` is kept, in `automation/tools/android/apksigner/`), and creates Kiwi's
  Android key if there is none yet;
- signs with APK Signature Schemes v2 and v3 (no v1: the app needs Android 8.0 or later),
  the key's password passed through the environment, never on a command line;
- verifies the result with `apksigner verify` and Kiwi's own verifier
  (`automation/apk.py`): the signature is Kiwi's certificate, package and version are
  unchanged, and uncompressed entries are aligned (apksigner aligns them itself);
- writes `android-app/kiwi.apk` and `android-app/kiwi.json`, each replaced in one step.

The phones see it as `Me.android_app`. The web app offers it to Android devices, and the
Android app offers its own update in My Apps when Kiwi serves a newer version.

When signing fails, Kiwi logs it, keeps offering the app it signed before, and shows a
warning on Android devices (in Settings) until an attempt succeeds. It tries again after
an hour, or at once when the input changes. The runner's own log is
`automation/kiwi-app.log` in the data directory. Turning Kiwi Builds off stops it.

## The pin

`automation/kiwi-app.json`:

```json
{
  "schema": 1,
  "package": "io.github.digitalpals.kiwi",
  "release": {
    "version_name": "1.0.0",
    "version_code": 1000000,
    "url": "https://github.com/DigitalPals/Kiwi/releases/download/android-v1.0.0/kiwi-android-1.0.0-unsigned.apk",
    "sha256": "<64 hex digits>",
    "size": 2021313
  }
}
```

`release` is `null` until the first release is published; Kiwi then downloads nothing and
only signs APKs you place yourself. The tests check the file's shape: the version code must
follow from the version, and the URL must be the release asset of tag
`android-v<version>`.

## Version numbers

App versions are `MAJOR.MINOR.PATCH`. The version code is
`MAJOR * 1000000 + MINOR * 1000 + PATCH` (1.0.0 is 1000000, 1.2.3 is 1002003), so a later
release always has a higher code; MINOR and PATCH stay below 1000. Gradle takes both from
properties:

```sh
./gradlew assembleRelease -PkiwiVersionName=1.2.3 -PkiwiVersionCode=1002003
```

Without them a build is 0.1.0 (code 1), which every release replaces.

## Releasing a new version

1. Merge the app's changes to `main` and wait for CI (the "Android app" job builds and
   tests it).
2. Tag the commit and push the tag:

   ```sh
   git switch main && git pull
   git tag -a android-v1.0.0 -m "Kiwi for Android 1.0.0"
   git push origin android-v1.0.0
   ```

   The **Android release** workflow (`.github/workflows/android-release.yml`) runs on the
   tag. You can also start it from the repository's Actions tab ("Run workflow" on
   `main`, version `1.0.0`); it then creates the tag on that commit.
3. The workflow derives the version name and code from the tag, runs the tests, builds the
   unsigned release APK, checks it with `automation/apk.py` and publishes the release
   "Kiwi for Android 1.0.0" with `kiwi-android-1.0.0-unsigned.apk` and
   `kiwi-android-1.0.0-unsigned.apk.sha256`. The release notes and the run's summary show
   the new `automation/kiwi-app.json`.
4. Optionally check the asset yourself:

   ```sh
   gh release download android-v1.0.0 --repo DigitalPals/Kiwi --dir /tmp/kiwi-android
   (cd /tmp/kiwi-android && sha256sum -c kiwi-android-1.0.0-unsigned.apk.sha256)
   ```

5. Commit that JSON as `automation/kiwi-app.json` and merge it. `python3 -m pytest
   automation/` and `cargo test` check it.
6. Update Kiwi as usual ([operations](operations.md#updating)). Each Kiwi signs and offers
   the new version within a minute of starting, and Kiwi's app on the phones offers the
   update.

Never replace a release's asset: the pinned SHA-256 would no longer match and Kiwis would
refuse it. Publish a new version instead.

## Your own builds

Until a release exists, or to try a change, build the app yourself and let Kiwi sign it:

1. Build an unsigned release APK (see [android/README.md](../android/README.md#build)),
   for example:

   ```sh
   android/scripts/build-in-docker.sh assembleRelease -PkiwiVersionName=1.0.1 -PkiwiVersionCode=1000001
   ```

2. Copy `android/app/build/outputs/apk/release/app-release-unsigned.apk` into
   `android-app/unsigned/` in Kiwi's data directory (any name ending in `.apk`; in Docker
   `docker compose cp app-release-unsigned.apk kiwi:/data/android-app/unsigned/`).

Within a minute Kiwi signs the APK with the highest version code there (the newest file
on a tie) and offers it. While that folder holds an APK, Kiwi does not use the pinned
release; remove the files to return to it. The APK must be unsigned: a debug build, or one
signed with another key, is refused and named in the warning.

You can also run the command by hand, for example to see its output; Kiwi then signs once
more at its next check, since the run did not come from it:

```sh
python3 "<data>/automation/android.py" kiwi-app --data-dir "<data>"
```

## Files

Paths are in Kiwi's data directory.

| Location | Purpose |
|---|---|
| `android-app/kiwi.apk`, `android-app/kiwi.json` | The signed app Kiwi offers and its description (`package`, `version_name`, `version_code`, `sha256`, `size`, plus `signer`, `source`, `unsigned_sha256`, `input` and `signed_at`, which the runner writes for itself) |
| `android-app/unsigned/` | Unsigned APKs you place to use instead of the pinned release |
| `automation/kiwi-app.json` | The pinned release, rewritten from Kiwi at every start |
| `automation/android/keystore.p12`, `keystore.pass` | Kiwi's Android key, shared with Kiwi Builds. [Back them up](operations.md#backups): with a new key, the phones have to uninstall Kiwi's app and its builds before they can install them again |
| `automation/tools/android/apksigner/` | `apksigner.jar` from the pinned Android SDK Build-Tools |
| `automation/kiwi-app.log` | The runner's private log (the previous one is kept as `kiwi-app.log.1`) |
