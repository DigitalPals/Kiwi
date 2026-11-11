#!/usr/bin/env python3
"""Kiwi Builds for Android. Speaks the same JSON-lines protocol as runner.py.

A build fetches the original app at the version the recipe pins, proves the
publisher signed every byte of it (apk.py), patches it with the pinned patcher
and patch bundle, and signs the result with a key that never leaves this
computer. Nothing here needs an Android device, a Google account or macOS.

The patcher and its patch bundle are third-party code, so they never see the
signing key: the patcher writes an unsigned APK, and Kiwi signs it afterwards
with the pinned apksigner. On Linux the patcher runs confined (sandbox.py): it
reads only its own tools and the original, writes only its working folder and
has no network. Every tool runs with a minimal environment, never Kiwi's own.

The original comes from the first source that can supply it:

1. `automation/android/originals/` - files the owner placed there by hand;
2. Google Play, through an anonymous session, at the pinned version code;
3. APKPure, at the pinned version.

A source is only a transport. Whatever it returns is accepted solely because
its APK signature verifies against the publisher certificate in the recipe.

The `kiwi-app` command prepares Kiwi's own Android app for the phones: it takes
the unsigned APK the project released (pinned in kiwi-app.json) or one the owner
placed in `android-app/unsigned/`, signs it with the same key and writes it to
`android-app/` for Kiwi's server to offer.
"""
import argparse
import base64
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import secrets
import shutil
import signal
import stat
import sys
import tarfile
import threading
import time
import urllib.error
import urllib.parse
import urllib.request
import zipfile

import apk
from runner import BuildError, Builder, canonical, digest, emit, recipe_key
import sandbox

RECIPES_PATH = Path(__file__).with_name("android.json")
KIWI_APP_PATH = Path(__file__).with_name("kiwi-app.json")
KIWI_APP_PACKAGE = "io.github.digitalpals.kiwi"
# A release asset of the project (or a fork) on GitHub, named after its version.
RELEASE_URL = re.compile(r"https://github\.com/[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+/releases/download/"
                         r"android-v(?P<tag>[^/]+)/kiwi-android-(?P<file>[^/]+)-unsigned\.apk")
MAX_KIWI_APP = 256 * 1024**2
PLAY = "https://android.clients.google.com/fdfe/"
PURE = "https://api.pureapk.com/m/v3/cms/app_version?hl=en-US&package_name="
MAX_APK = 1024**3
BUNDLES = (".apks", ".xapk", ".apkm")
ALIAS = "kiwi"
SESSION_SECONDS = 1800
# The JVM options of every tool Kiwi runs: no shared-memory statistics in /tmp, no attach socket.
JVM = ("-XX:-UsePerfData", "-XX:+DisableAttachMechanism")
MAX_RESULT = 16 * 1024**2
# Each file Play delivers is tried this often, with a growing pause between tries.
DOWNLOAD_ATTEMPTS, RETRY_SECONDS = 3, 5
# hashlib.file_digest (3.11) and tarfile's extraction filters (3.12).
MIN_PYTHON = (3, 12)


class SourceError(Exception):
    """One source could not supply a trustworthy original; the next is tried."""


def require_python(version=sys.version_info):
    """Stops plainly on a Python too old for the builder, before anything needs it."""
    if tuple(version[:2]) < MIN_PYTHON:
        found = ".".join(str(part) for part in version[:3])
        raise BuildError(f"Kiwi Builds for Android needs Python {MIN_PYTHON[0]}.{MIN_PYTHON[1]} or newer, "
                         f"but Kiwi runs it with Python {found}. Install a newer Python and start Kiwi with "
                         "--python (KIWI_PYTHON) naming it.", True)


def version_code(name):
    """The Android version code of a Kiwi app release `MAJOR.MINOR.PATCH`.

    MAJOR * 1000000 + MINOR * 1000 + PATCH, so every later release has a higher code.
    The release workflow (.github/workflows/android-release.yml) uses the same rule.
    """
    match = re.fullmatch(r"(0|[1-9][0-9]{0,3})\.(0|[1-9][0-9]{0,2})\.(0|[1-9][0-9]{0,2})", name or "")
    if not match:
        raise ValueError(f"{name!r} is not a version like 1.2.3")
    major, minor, patch = (int(part) for part in match.groups())
    code = major * 1_000_000 + minor * 1_000 + patch
    if not 0 < code <= 2_100_000_000:
        raise ValueError(f"version {name} is out of range")
    return code


def check_pin(pin):
    """The pinned release in kiwi-app.json, or None before the first one. Raises ValueError
    when the file does not have the expected shape."""
    if not isinstance(pin, dict) or set(pin) != {"schema", "package", "release"}:
        raise ValueError("kiwi-app.json must hold exactly schema, package and release")
    if pin["schema"] != 1 or pin["package"] != KIWI_APP_PACKAGE:
        raise ValueError(f"kiwi-app.json must be schema 1 for {KIWI_APP_PACKAGE}")
    release = pin["release"]
    if release is None:
        return None
    if not isinstance(release, dict) or set(release) != {"version_name", "version_code", "url", "sha256", "size"}:
        raise ValueError("the pinned release must hold exactly version_name, version_code, url, sha256 and size")
    name = release["version_name"]
    if not isinstance(name, str) or type(release["version_code"]) is not int or release["version_code"] != version_code(name):
        raise ValueError("the pinned version_code must follow from version_name (MAJOR*1000000 + MINOR*1000 + PATCH)")
    match = RELEASE_URL.fullmatch(release["url"]) if isinstance(release["url"], str) else None
    if not match or match["tag"] != name or match["file"] != name:
        raise ValueError(f"the pinned url must be the release asset kiwi-android-{name}-unsigned.apk of tag android-v{name}")
    if not isinstance(release["sha256"], str) or not re.fullmatch(r"[0-9a-f]{64}", release["sha256"]):
        raise ValueError("the pinned sha256 must be 64 lowercase hex digits")
    if type(release["size"]) is not int or not 0 < release["size"] <= MAX_KIWI_APP:
        raise ValueError("the pinned size must be the APK's size in bytes")
    return release


def records(data):
    """The (field, value) pairs of one protobuf message; bytes stay undecoded."""
    out, at = [], 0

    def varint():
        nonlocal at
        value = shift = 0
        while True:
            byte = data[at]
            at += 1
            value |= (byte & 0x7F) << shift
            shift += 7
            if not byte & 0x80:
                return value
            if shift > 63:
                raise ValueError("varint")

    try:
        while at < len(data):
            key = varint()
            kind = key & 7
            if kind == 0:
                value = varint()
            elif kind == 2:
                size = varint()
                value = data[at:at + size]
                at += size
            elif kind in (1, 5):
                size = 8 if kind == 1 else 4
                value = data[at:at + size]
                at += size
            else:
                raise ValueError("wire type")
            if at > len(data) or key >> 3 == 0:
                raise ValueError("truncated")
            out.append((key >> 3, value))
    except (IndexError, ValueError) as error:
        raise SourceError("it sent an unreadable reply") from error
    return out


def nested(data, *path):
    """Every value at a field path; missing fields give an empty list."""
    found = [data]
    for field in path:
        found = [value for item in found if isinstance(item, bytes)
                 for number, value in records(item) if number == field]
    return found


def first(data, *path):
    found = nested(data, *path)
    return found[0] if found else None


def text(value):
    return value.decode("utf-8", "replace") if isinstance(value, bytes) else ""


def http(url, *, headers=None, body=None, limit=8 * 1024**2, timeout=60):
    """A bounded HTTPS exchange. Returns (status, bytes)."""
    if not url.startswith("https://"):
        raise SourceError("it offered an insecure address")
    request = urllib.request.Request(url, data=body, headers={"User-Agent": "Kiwi", **(headers or {})},
                                     method="POST" if body is not None else "GET")
    try:
        with urllib.request.urlopen(request, timeout=timeout) as response:
            data = response.read(limit + 1)
            status = response.status
    except urllib.error.HTTPError as error:
        return error.code, error.read(64 * 1024)
    except (urllib.error.URLError, OSError, ValueError) as error:
        raise SourceError("it could not be reached") from error
    if len(data) > limit:
        raise SourceError("it sent an oversized reply")
    return status, data


def download(url, target, *, limit=MAX_APK, headers=None, sha256=None):
    """Streams an HTTPS download to a file and returns its SHA-256."""
    if not url.startswith("https://"):
        raise SourceError("it offered an insecure address")
    request = urllib.request.Request(url, headers={"User-Agent": "Kiwi", **(headers or {})})
    hasher, total = hashlib.sha256(), 0
    try:
        with urllib.request.urlopen(request, timeout=60) as response, open(target, "wb") as out:
            while chunk := response.read(1024 * 1024):
                total += len(chunk)
                if total > limit:
                    raise SourceError("it sent an oversized file")
                hasher.update(chunk)
                out.write(chunk)
    except (urllib.error.URLError, OSError, ValueError) as error:
        target.unlink(missing_ok=True)
        raise SourceError("a download failed") from error
    except SourceError:
        target.unlink(missing_ok=True)
        raise
    if sha256 and hasher.hexdigest() != sha256:
        target.unlink(missing_ok=True)
        raise SourceError("a download did not match its pinned checksum")
    return hasher.hexdigest()


def unpack_bundle(path, target):
    """The APKs of a split-APK bundle (.apks, .xapk, .apkm), extracted flat."""
    target.mkdir(mode=0o700, parents=True, exist_ok=True)
    files = []
    try:
        with zipfile.ZipFile(path) as archive:
            members = [e for e in archive.infolist() if e.filename.lower().endswith(".apk") and not e.is_dir()]
            if not members or len(members) > 200 or sum(e.file_size for e in members) > 2 * MAX_APK:
                raise SourceError("the bundle holds no usable packages")
            for index, entry in enumerate(members):
                out = target / f"{index:03d}.apk"
                with archive.open(entry) as source, open(out, "wb") as sink:
                    shutil.copyfileobj(source, sink, 1024 * 1024)
                files.append(out)
    except (zipfile.BadZipFile, OSError) as error:
        raise SourceError("the bundle is unreadable") from error
    return files


class Play:
    """Google Play's delivery API, with an anonymous session.

    Play serves any published version to a session that asks for its version
    code. The session comes from a public token dispenser, so no personal
    Google account is involved; it only authorises downloads of free apps.
    """

    def __init__(self, settings, device, cache):
        self.settings, self.device, self.cache = settings, device, cache
        self.state = None

    def session(self, fresh=False):
        if self.state and not fresh:
            return self.state
        if not fresh:
            try:
                saved = json.loads(self.cache.read_text())
                if time.time() - saved["fetched_at"] < SESSION_SECONDS:
                    self.state = saved
                    return saved
            except (OSError, ValueError, KeyError, TypeError):
                pass
        status, body = http(self.settings["dispenser"], body=json.dumps(self.device).encode(),
                            headers={"User-Agent": self.settings["agent"], "Content-Type": "application/json"})
        if status == 429:
            raise SourceError("its anonymous sign-in is rate limited; try again later")
        try:
            reply = json.loads(body) if status == 200 else {}
            state = {"token": reply["authToken"], "device": reply["gsfId"],
                     "checkin": reply.get("deviceCheckInConsistencyToken") or "",
                     "config": reply.get("deviceConfigToken") or "", "cookie": reply.get("dfeCookie") or "",
                     "agent": reply["deviceInfoProvider"]["userAgentString"],
                     "network": reply["deviceInfoProvider"].get("mccMnc") or "", "fetched_at": time.time()}
            if not all(isinstance(state[k], str) for k in ("token", "device", "checkin", "config", "cookie", "agent", "network")):
                raise ValueError
        except (ValueError, KeyError, TypeError) as error:
            raise SourceError("its anonymous sign-in is unavailable") from error
        self.cache.write_text(json.dumps(state))
        self.cache.chmod(0o600)
        self.state = state
        return state

    def call(self, endpoint, params, post=False):
        for attempt in range(2):
            state = self.session(fresh=attempt == 1)
            locales = [x for x in self.device.get("Locales", "en_US").split(",") if x]
            headers = {
                "Authorization": "Bearer " + state["token"], "User-Agent": state["agent"],
                "X-DFE-Device-Id": state["device"], "Accept-Language": "en-US",
                "X-DFE-Client-Id": "am-android-google", "X-DFE-Network-Type": "4",
                "X-DFE-Content-Filters": "", "X-Limit-Ad-Tracking-Enabled": "false", "X-Ad-Id": "",
                # Every language the device profile lists: Play then offers each
                # language's resources, so the app follows the phone's language.
                "X-DFE-UserLanguages": ",".join(locales), "X-DFE-Request-Params": "timeoutMs=4000",
            }
            for name, key in (("X-DFE-Device-Checkin-Consistency-Token", "checkin"), ("X-DFE-Device-Config-Token", "config"),
                              ("X-DFE-Cookie", "cookie"), ("X-DFE-MCCMNC", "network")):
                if state[key]:
                    headers[name] = state[key]
            status, body = http(PLAY + endpoint + "?" + urllib.parse.urlencode(params), headers=headers,
                                body=b"" if post else None)
            if status == 200:
                return body
            if status not in (401, 403):
                break
        raise SourceError("it refused the request" if status in (401, 403, 404) else "it is unavailable")

    def fetch(self, package, code, target):
        params = {"ot": "1", "doc": package, "vc": str(code)}
        token = first(self.call("purchase", params, post=True), 1, 4, 55)
        if token:
            params["dtok"] = text(token)
        data = first(self.call("delivery", params), 1, 21, 2)
        url = text(first(data, 3)) if data else ""
        if not url:
            raise SourceError("it does not offer this version")
        wanted = [("base", url, first(data, 1))]
        for split in nested(data, 15):
            name, link = text(first(split, 1)), text(first(split, 5))
            if not re.fullmatch(r"[A-Za-z0-9_.]{1,80}", name) or not link:
                raise SourceError("it sent an unreadable reply")
            wanted.append((name, link, first(split, 2)))
        files = []
        for name, link, size in wanted:
            path = target / f"{name}.apk"
            limit = size + 1024 if isinstance(size, int) and 0 < size < MAX_APK else MAX_APK
            # Each file is retried on its own: an app can have 80 splits, and one failed
            # download shouldn't fetch them all again.
            for attempt in range(DOWNLOAD_ATTEMPTS):
                try:
                    download(link, path, limit=limit)
                    break
                except SourceError:
                    if attempt == DOWNLOAD_ATTEMPTS - 1:
                        raise
                    time.sleep(RETRY_SECONDS * (attempt + 1))
            files.append(path)
        return files


def apkpure(package, version, code, abi, target):
    status, body = http(PURE + urllib.parse.quote(package), limit=32 * 1024**2,
                        headers={"x-cv": "3172501", "x-sv": "29", "x-abis": abi, "x-gp": "1"})
    if status != 200:
        raise SourceError("it is unavailable")
    for entry in nested(body, 1, 7, 2):
        info = first(entry, 3, 2)
        if not info or text(first(info, 5)) != str(code) or text(first(info, 6)) != version:
            continue
        asset = first(info, 24)
        kind, url = text(first(asset, 8)) if asset else "", text(first(asset, 9)) if asset else ""
        if kind not in ("APK", "XAPK") or not url:
            continue
        path = target / ("original.apk" if kind == "APK" else "original.xapk")
        download(url, path)
        return [path] if kind == "APK" else unpack_bundle(path, target / "bundle")
    raise SourceError("it does not offer this version")


class Android:
    def __init__(self, builder):
        self.builder = builder
        self.recipes = json.loads(RECIPES_PATH.read_text())
        self.toolchain = self.recipes["toolchain"]
        self.tools = builder.tools / "android"
        self.private = builder.root / "android"
        for path in (self.tools, self.private, self.private / "originals"):
            path.mkdir(mode=0o700, parents=True, exist_ok=True)

    def app(self, app_id):
        for app in self.recipes["apps"]:
            if app["id"] == app_id:
                return app
        raise BuildError("Unknown app recipe")

    def sources_digest(self):
        """Everything outside the recipe that decides what a build produces."""
        return recipe_key({"android": digest(Path(__file__)), "apk": digest(Path(apk.__file__)),
                           "device": recipe_key(self.recipes["device"])})

    def build_toolchain(self):
        """The pinned tools a patched app depends on: all of them, apksigner included, which
        signs the patched apps too."""
        return dict(self.toolchain)

    def sandbox_mode(self, app):
        """How strictly the patcher must be confined for this app. The pinned official patch
        bundle runs confined where the computer allows it; a recipe whose patches come from
        less trusted publishers says "required" and is never built unconfined."""
        mode = app.get("sandbox", sandbox.BEST_EFFORT)
        if mode not in sandbox.MODES:
            raise BuildError("Unknown sandbox setting in the app recipe")
        return mode

    def confined(self, argv, *, mode, read=(), run=(), write=(), env, **kwargs):
        """Runs a tool through the sandbox; any process it left behind is ended afterwards."""
        try:
            wrapped = sandbox.command(argv, mode=mode, read=read, run=run, write=write, log=self.builder.log)
        except sandbox.SandboxError as error:
            raise BuildError(str(error), True) from error
        try:
            return self.builder.command(wrapped, env=env, **kwargs)
        finally:
            if strays := sandbox.kill_strays():
                self.builder.log.write(f"kiwi: ended {strays} process(es) the tool left running\n".encode())

    # --- pinned tools ------------------------------------------------------

    def java_archive(self):
        system = {"darwin": "mac", "linux": "linux"}.get(sys.platform)
        machine = {"x86_64": "x86_64", "amd64": "x86_64", "arm64": "aarch64", "aarch64": "aarch64"}.get(platform.machine().lower())
        spec = self.toolchain["java"]["archives"].get(f"{system}-{machine}")
        if spec is None:
            raise BuildError("Android builds are not supported on this computer.", True)
        return spec

    def pinned(self, spec, target):
        try:
            download(spec["url"], target, limit=spec["size"], sha256=spec["sha256"])
        except SourceError as error:
            raise BuildError(f"Kiwi could not fetch a pinned build tool: {error}.") from error

    def java(self):
        spec = self.java_archive()
        home = self.tools / "java" / spec["sha256"][:16]
        if not (home.with_name(home.name + ".complete")).is_file():
            emit("stage", stage="Fetching the pinned Java runtime")
            partial = home.with_name(home.name + ".partial")
            for stale in (home, partial):
                if stale.exists():
                    shutil.rmtree(stale)
            partial.mkdir(mode=0o700, parents=True)
            archive = partial.with_name(home.name + ".tar.gz")
            self.pinned(spec, archive)
            try:
                with tarfile.open(archive) as bundle:
                    bundle.extractall(partial, filter="data")
            except (tarfile.TarError, OSError) as error:
                raise BuildError("The pinned Java runtime could not be unpacked.") from error
            finally:
                archive.unlink(missing_ok=True)
            partial.rename(home)
            home.with_name(home.name + ".complete").write_text(spec["sha256"])
        found = [p for pattern in ("*/bin/java", "*/Contents/Home/bin/java") for p in home.glob(pattern)]
        if len(found) != 1:
            raise BuildError("The pinned Java runtime is incomplete; delete Kiwi's Android tools and retry.")
        return found[0]

    def tool(self, name):
        spec = self.toolchain[name]
        target = self.tools / name / spec["sha256"][:16] / Path(urllib.parse.urlparse(spec["url"]).path).name
        if not target.is_file() or digest(target) != spec["sha256"]:
            emit("stage", stage=f"Fetching {spec['name']}")
            target.parent.mkdir(mode=0o700, parents=True, exist_ok=True)
            partial = target.with_name(target.name + ".partial")
            self.pinned(spec, partial)
            partial.replace(target)
        return target

    def apksigner(self):
        """apksigner.jar from the pinned Android build-tools. Only the jar is kept: it runs on
        the pinned Java runtime, on every platform."""
        spec = self.toolchain["apksigner"]
        home = self.tools / "apksigner" / spec["sha256"][:16]
        jar = home / "apksigner.jar"
        if not home.with_name(home.name + ".complete").is_file() or not jar.is_file():
            emit("stage", stage=f"Fetching {spec['name']}")
            partial = home.with_name(home.name + ".partial")
            for stale in (home, partial):
                if stale.exists():
                    shutil.rmtree(stale)
            partial.mkdir(mode=0o700, parents=True)
            archive = partial / "build-tools.zip"
            self.pinned(spec, archive)
            try:
                with zipfile.ZipFile(archive) as bundle:
                    entry = bundle.getinfo(spec["member"])
                    if entry.file_size > 64 * 1024**2:
                        raise BuildError("The pinned apksigner is unexpectedly large.")
                    # Only this one member is written, to a name Kiwi chooses.
                    with bundle.open(entry) as source, open(partial / "apksigner.jar", "wb") as sink:
                        shutil.copyfileobj(source, sink, 1024 * 1024)
            except (KeyError, zipfile.BadZipFile, OSError) as error:
                raise BuildError("The pinned Android build-tools could not be unpacked.") from error
            finally:
                archive.unlink(missing_ok=True)
            partial.rename(home)
            home.with_name(home.name + ".complete").write_text(spec["sha256"])
        return jar

    # --- signing key -------------------------------------------------------

    def keystore(self, java):
        """Kiwi's Android signing key: made once, kept beside the other private state.

        Android only installs an update over an app signed with the same key, so
        this key is the identity of every app Kiwi builds for this owner.
        """
        store, secret = self.private / "keystore.p12", self.private / "keystore.pass"
        recorded = self.private / "signer.json"
        keytool = java.with_name("keytool")
        if not store.is_file() and recorded.exists():
            # signer.json is written whenever the key loads, before anything is signed with
            # it: a key existed. A new one would quietly break every update on the phones.
            raise BuildError("Kiwi's Android signing key (automation/android/keystore.p12) is missing. Restore "
                             "keystore.p12 and keystore.pass from a backup. To start over with a new key instead, "
                             "delete automation/android/signer.json; every phone then has to uninstall Kiwi's "
                             "Android apps before it can install them again.", True)
        if not store.is_file():
            emit("stage", stage="Creating Kiwi's Android signing key")
            password = secrets.token_hex(24)
            partial = store.with_name("keystore.partial")
            partial.unlink(missing_ok=True)
            self.builder.command([keytool, "-genkeypair", "-keystore", partial, "-storetype", "PKCS12", "-alias", ALIAS,
                                  "-keyalg", "RSA", "-keysize", "4096", "-sigalg", "SHA256withRSA", "-validity", "36500",
                                  "-dname", "CN=Kiwi", "-storepass:env", "KIWI_KEYSTORE", "-keypass:env", "KIWI_KEYSTORE"],
                                 env=sandbox.environment(self.private, self.private, KIWI_KEYSTORE=password),
                                 message="Kiwi could not create its Android signing key.")
            secret.write_text(password)
            secret.chmod(0o600)
            partial.chmod(0o600)
            partial.rename(store)
        try:
            password = secret.read_text().strip()
        except OSError as error:
            raise BuildError("Kiwi's Android signing key has no password file; restore both from a backup.", True) from error
        pem = self.builder.command([keytool, "-exportcert", "-rfc", "-keystore", store, "-alias", ALIAS,
                                    "-storepass:env", "KIWI_KEYSTORE"],
                                   env=sandbox.environment(self.private, self.private, KIWI_KEYSTORE=password),
                                   capture=True, message="Kiwi's Android signing key could not be read.")
        body = re.search(r"-----BEGIN CERTIFICATE-----(.+?)-----END CERTIFICATE-----", pem, re.S)
        if not body:
            raise BuildError("Kiwi's Android signing key could not be read.")
        signer = hashlib.sha256(base64.b64decode(body.group(1))).hexdigest()
        before = None
        if recorded.exists():
            try:
                before = json.loads(recorded.read_text())["sha256"]
                if not isinstance(before, str) or not re.fullmatch(r"[0-9a-fA-F]{64}", before):
                    raise ValueError(before)
            except (OSError, ValueError, KeyError, TypeError) as error:
                # It can't vouch for the key, so neither can Kiwi: a wrong key would pass.
                raise BuildError("automation/android/signer.json is damaged, so Kiwi can't check that "
                                 "keystore.p12 is the key it signed its Android apps with. Restore "
                                 "signer.json from a backup, or delete it if keystore.p12 is the right "
                                 "key.", True) from error
        if before and before.lower() != signer:
            raise BuildError(f"automation/android/keystore.p12 is not the key Kiwi signed its Android apps with "
                             f"(its certificate is {signer}, not {before.lower()}). Restore the right keystore.p12 "
                             "and keystore.pass from a backup. To switch to this key instead, delete "
                             "automation/android/signer.json; every phone then has to uninstall Kiwi's Android "
                             "apps before it can install them again.", True)
        # Not a secret: Kiwi's server reads it to tell its own builds on a phone from others.
        partial = self.private / "signer.partial"
        partial.write_bytes(canonical({"sha256": signer}))
        partial.replace(recorded)
        return store, password, signer

    # --- the original app --------------------------------------------------

    def verified(self, app, plan, files):
        """Every file is the publisher's own, for the planned app and version."""
        code, base, abis = int(plan["external_id"]), 0, set()
        for path in files:
            try:
                facts = apk.inspect(path)
            except (apk.ApkError, OSError) as error:
                raise SourceError(str(error)) from error
            if facts["package"] != app["original_package"]:
                raise SourceError("it supplied a different app")
            if app["certificate"] not in facts["certificates"]:
                raise SourceError(f"its file is not signed by {app['publisher']}")
            if facts["version_code"] != code:
                raise SourceError("it supplied a different version")
            abis.update(facts["abis"])
            if "split" not in facts:
                base += 1
                if facts["version"] != plan["version"]:
                    raise SourceError("it supplied a different version")
        if base != 1:
            raise SourceError("it supplied an incomplete app")
        if abis and self.toolchain["abi"] not in abis:
            raise SourceError("it supplied the app for a different processor")

    def dropped(self, app, plan, target):
        """Originals the owner placed in automation/android/originals by hand."""
        for path in sorted((self.private / "originals").iterdir()):
            if not path.is_file() or path.suffix.lower() not in (".apk", *BUNDLES):
                continue
            try:
                work = target / path.stem
                files = [path] if path.suffix.lower() == ".apk" else unpack_bundle(path, work)
                self.verified(app, plan, files)
                return files
            except SourceError:
                shutil.rmtree(target / path.stem, ignore_errors=True)
        return None

    def original(self, app, plan, job_dir):
        """(files, source name) for the planned version, from the first source that has it."""
        code = int(plan["external_id"])
        cache = self.tools / "originals" / app["original_package"] / str(code)
        if cache.with_name(cache.name + ".complete").is_file():
            files = sorted(cache.glob("*.apk"))
            try:
                self.verified(app, plan, files)
                return files, cache.with_name(cache.name + ".complete").read_text()
            except SourceError:
                pass
        play = Play(self.toolchain["play"], self.recipes["device"], self.private / "play-session.json")
        sources = {
            "play": ("Google Play", lambda target: play.fetch(app["original_package"], code, target)),
            "apkpure": ("APKPure", lambda target: apkpure(app["original_package"], plan["version"], code, self.toolchain["abi"], target)),
        }
        attempts = [("the originals folder", lambda target: self.dropped(app, plan, target))]
        attempts += [sources[name] for name in self.toolchain["sources"]]
        failures = []
        for index, (name, fetch) in enumerate(attempts):
            target = job_dir / f"original-{index}"
            target.mkdir(mode=0o700)
            try:
                files = fetch(target)
                if not files:
                    shutil.rmtree(target, ignore_errors=True)
                    continue
                self.verified(app, plan, files)
            except SourceError as error:
                failures.append(f"{name}: {error}")
                self.builder.log.write(f"kiwi: {name}: {error}\n".encode())
                shutil.rmtree(target, ignore_errors=True)
                continue
            # Keep the verified original: a rebuild with other options, or after a
            # patch update for the same version, then needs no download at all.
            for stale in cache.parent.glob("*"):
                shutil.rmtree(stale) if stale.is_dir() else stale.unlink()
            cache.mkdir(mode=0o700, parents=True)
            kept = []
            for number, path in enumerate(files):
                kept.append(cache / f"{number:03d}.apk")
                shutil.copyfile(path, kept[-1])
            cache.with_name(cache.name + ".complete").write_text(name)
            shutil.rmtree(target, ignore_errors=True)
            return kept, name
        raise BuildError(f"No source could supply the original {app['name']} {plan['version']} ("
                         + "; ".join(failures) + "). Kiwi will retry; you can also place the APK in "
                         "automation/android/originals on the Kiwi computer.")

    # --- plan and build ----------------------------------------------------

    def options(self, app, options):
        optional = [p["id"] for p in app["packages"] if p.get("optional")]
        tweaks = sorted(set(options.get("tweaks", [])))
        if any(t not in optional for t in tweaks):
            raise BuildError("Unknown tweak selection")
        if options.get("name") is not None:
            raise BuildError("Unknown app name")
        return tweaks

    def plan(self, app, options):
        self.java_archive()
        plan = {"app": app["id"], "platform": "android", "external_id": str(app["version_code"]),
                "version": app["app_version"], "recipe": {"toolchain": self.build_toolchain(), "app": app},
                "tweaks": self.options(app, options), "name": None, "runner_sha256": self.sources_digest()}
        plan["key"] = recipe_key(plan)
        emit("plan", plan=plan)

    def build(self, app, plan, sequence, job_dir):
        expected_key = plan["key"]
        if (expected_key != recipe_key({k: v for k, v in plan.items() if k != "key"}) or plan.get("app") != app["id"]
                or plan["recipe"] != {"toolchain": self.build_toolchain(), "app": app} or plan["runner_sha256"] != self.sources_digest()):
            raise BuildError("The build plan changed; check for updates again")
        if not isinstance(sequence, int) or not 0 < sequence < 2**53:
            raise BuildError("Invalid Kiwi build revision")
        tweaks = self.options(app, plan)
        mode = self.sandbox_mode(app)
        java = self.java()
        patches = self.tool("patches")
        patcher = self.tool("patcher")
        signer_jar = self.apksigner()
        store, password, signer = self.keystore(java)
        emit("stage", stage=f"Downloading original {app['name']}")
        files, source = self.original(app, plan, job_dir)
        original_sha = recipe_key([digest(p) for p in files])
        if len(files) == 1:
            original = files[0]
        else:
            original = job_dir / "original.apks"
            with zipfile.ZipFile(original, "w", zipfile.ZIP_STORED) as bundle:
                for path in files:
                    bundle.write(path, path.name)
        emit("stage", stage=f"Patching {app['name']}")
        # The patcher's only writable folder. It gets the original, its own jar and the patch
        # bundle to read, and nothing of Kiwi's: no key, no password, no environment.
        box = job_dir / "patch"
        for name in ("home", "tmp", "data"):
            (box / name).mkdir(mode=0o700, parents=True)
        # Patches the recipe leaves out, such as the one that replaces the app's own icon.
        selection = [arg for name in app.get("disabled_patches", []) for arg in ("--disable", name)]
        for package in app["packages"]:
            for name in package.get("patches", []) if package.get("optional") else []:
                selection += ["--enable" if package["id"] in tweaks else "--disable", name]
        self.confined(
            [java, *JVM, f"-Djava.io.tmpdir={box / 'tmp'}", f"-Duser.home={box / 'home'}", "-jar", patcher, "patch",
             "--patches", patches, "--out", box / "unsigned.apk", "--temporary-files-path", box / "work",
             "--result-file", box / "result.json", "--unsigned", "--striplibs", self.toolchain["abi"], *selection, original],
            mode=mode, read=[patcher, patches, original], run=[java.parent.parent], write=[box],
            cwd=box, timeout=3600,
            # The patcher records every app it patches; keep that inside the job.
            env=sandbox.environment(box / "home", box / "tmp", MORPHE_DATA_DIR=box / "data"),
            message=f"Patching failed. {app['name']} {plan['version']} may need an updated patch recipe.")
        # What the patcher left is read only as plain files moved out of its folder: a link
        # could otherwise point Kiwi at a file of its own.
        unsigned, result = job_dir / "unsigned.apk", job_dir / "result.json"
        for made, kept in ((box / "unsigned.apk", unsigned), (box / "result.json", result)):
            try:
                if not stat.S_ISREG(os.lstat(made).st_mode):
                    raise OSError("not a file")
                os.replace(made, kept)
            except OSError as error:
                raise BuildError("The patcher reported no result" if made.name == "result.json"
                                 else "The patcher produced no app") from error
        try:
            if result.stat().st_size > MAX_RESULT:
                raise ValueError("oversized")
            outcome = json.loads(result.read_text())
            if not isinstance(outcome, dict):
                raise ValueError("not an object")
        except (OSError, ValueError) as error:
            raise BuildError("The patcher reported no result") from error
        if outcome.get("success") is not True or outcome.get("failedPatches") or not outcome.get("appliedPatches"):
            raise BuildError(f"Some patches did not apply to {app['name']} {plan['version']}; nothing was published")
        if not isinstance(outcome["appliedPatches"], list) or not all(isinstance(p, str) for p in outcome["appliedPatches"]):
            raise BuildError("The patcher reported an unreadable result")
        try:
            before = apk.unsigned(unsigned)
        except (apk.ApkError, OSError) as error:
            raise BuildError(f"The patched app is not a valid unsigned package: {error}") from error
        emit("stage", stage=f"Signing {app['name']}")
        candidate = job_dir / "candidate.apk"
        # The password reaches apksigner through its environment, never its arguments.
        self.confined(
            [java, *JVM, f"-Djava.io.tmpdir={job_dir}", "-jar", signer_jar, "sign", "--ks", store, "--ks-type", "PKCS12",
             "--ks-key-alias", ALIAS, "--ks-pass", "env:KIWI_KEYSTORE", "--key-pass", "env:KIWI_KEYSTORE",
             "--v1-signing-enabled", "false", "--v2-signing-enabled", "true", "--v3-signing-enabled", "true",
             "--v4-signing-enabled", "false", "--out", candidate, unsigned],
            mode=mode, read=[signer_jar, store, unsigned], run=[java.parent.parent], write=[job_dir],
            cwd=job_dir, timeout=600, env=sandbox.environment(job_dir, job_dir, KIWI_KEYSTORE=password),
            message="apksigner could not sign the patched app; details are in the private log.")
        emit("stage", stage="Validating the patched app")
        try:
            patched = apk.inspect(candidate)
            unaligned = apk.misaligned(candidate)
        except (apk.ApkError, OSError) as error:
            raise BuildError("The patched app is not a valid signed package") from error
        self.builder.log.write(f"kiwi: patched app: {json.dumps(patched)}\n".encode())
        if patched["certificates"] != [signer]:
            raise BuildError("The patched app is not signed with Kiwi's key")
        if not {2, 3} <= set(patched["schemes"]):
            raise BuildError("The patched app lacks an APK signature v2 or v3")
        if unaligned:
            raise BuildError("The patched app is not aligned (" + ", ".join(unaligned[:3]) + ")")
        if any(patched[k] != before[k] for k in ("package", "version", "version_code", "abis")):
            raise BuildError("Signing changed the patched app unexpectedly")
        # Some patches deliberately raise the version code so that a store cannot
        # replace the app; the recipe then pins the code the patched app carries.
        if (patched["package"] != app["bundle_id"] or patched["version"] != plan["version"]
                or patched["version_code"] != app.get("patched_version_code", app["version_code"])):
            raise BuildError(f"Patching changed {app['name']}’s identity or version unexpectedly")
        if patched["abis"] and self.toolchain["abi"] not in patched["abis"]:
            raise BuildError("The patched app lost its native code")
        sha = digest(candidate)
        artifact = self.builder.root / "artifacts" / f"{sha}.apk"
        os.replace(candidate, artifact)
        provenance = {"plan": plan, "sequence": sequence, "source": source, "original_sha256": original_sha,
                      "sha256": sha, "built_at": time.time(), "signer": signer,
                      "patches": sorted(outcome["appliedPatches"]), "runtime_tested": False}
        (self.builder.root / "artifacts" / f"{sha}.json").write_bytes(canonical(provenance))
        emit("artifact", sha256=sha, version=patched["version"], build=str(patched["version_code"]), revision=sequence,
             min_os=None, size=artifact.stat().st_size, package=patched["package"], signer=signer,
             notes=f"{app['name']} {plan['version']} · Android build {plan['external_id']}\n"
                   f"Original from {source}, verified against {app['publisher']}’s signing certificate.\n"
                   f"{len(outcome['appliedPatches'])} patches from {self.toolchain['patches']['name']}; signed with this Kiwi’s own key.\n"
                   f"Recipe {expected_key[:12]}. Not yet tested on a physical device.")

    # --- Kiwi's own Android app ---------------------------------------------

    def kiwi_app_input(self, job_dir):
        """(file, manifest, source) of the unsigned app to sign: the newest one the owner
        placed in android-app/unsigned/, unless the pinned release is newer (so a test build
        left there never holds back a release), else the pinned release, downloaded and checked."""
        folder = self.builder.root.parent / "android-app" / "unsigned"
        found, problems = [], []
        if folder.is_dir():
            for path in sorted(folder.iterdir()):
                if path.suffix.lower() != ".apk" or not path.is_file():
                    continue
                try:
                    facts = apk.unsigned(path)
                except (apk.ApkError, OSError) as error:
                    problems.append(f"{path.name}: {error}")
                    continue
                if facts["package"] != KIWI_APP_PACKAGE:
                    problems.append(f"{path.name}: it is not Kiwi's app but {facts['package']}")
                    continue
                found.append((facts["version_code"], path.stat().st_mtime, path, facts))
        for problem in problems:
            self.builder.log.write(f"kiwi: android-app/unsigned/{problem}\n".encode())
        local = max(found, key=lambda item: item[:2]) if found else None
        try:
            release, pin_error = check_pin(json.loads(KIWI_APP_PATH.read_text())), None
        except (OSError, ValueError) as error:
            release, pin_error = None, error
        if local and (release is None or local[0] >= release["version_code"]):
            _, _, path, facts = local
            return path, facts, "local"
        if problems and not local:
            raise BuildError("No APK in android-app/unsigned can be used (" + "; ".join(problems) + "). Place the "
                             "unsigned release APK of Kiwi's app there, or remove the files to use the released app.", True)
        if pin_error is not None:
            raise BuildError(f"Kiwi's Android app pin is not valid: {pin_error}.") from pin_error
        if release is None:
            raise BuildError("No release of Kiwi's Android app is pinned yet. To use one you built, place its "
                             "unsigned APK in android-app/unsigned.", True)
        emit("stage", stage=f"Downloading Kiwi for Android {release['version_name']}")
        path = job_dir / "unsigned.apk"
        try:
            download(release["url"], path, limit=release["size"], sha256=release["sha256"])
        except SourceError as error:
            raise BuildError(f"Kiwi could not fetch its Android app {release['version_name']}: {error}.") from error
        try:
            if path.stat().st_size != release["size"]:
                raise apk.ApkError("it does not have the pinned size")
            facts = apk.unsigned(path)
        except (apk.ApkError, OSError) as error:
            raise BuildError(f"The released Kiwi Android app {release['version_name']} is not usable: {error}.") from error
        if (facts["package"], facts["version_code"], facts["version"]) != (
                KIWI_APP_PACKAGE, release["version_code"], release["version_name"]):
            raise BuildError("The released Kiwi Android app is not the pinned version.")
        return path, facts, "release"

    def kiwi_app(self, request, job_dir):
        """Signs Kiwi's Android app with this Kiwi's key and writes it where the server offers it.

        Kiwi's own builds and Kiwi's app share the key, so the app can update the builds
        silently and is updated itself like them.
        """
        emit("stage", stage="Looking for Kiwi's Android app")
        source_path, facts, source = self.kiwi_app_input(job_dir)
        if not facts["version"] or len(facts["version"]) > 64 or not facts["version"].isprintable():
            raise BuildError("Kiwi's Android app has no usable versionName.")
        unsigned_sha = digest(source_path)
        java = self.java()
        signer_jar = self.apksigner()
        store, password, signer = self.keystore(java)
        emit("stage", stage=f"Signing Kiwi for Android {facts['version']}")
        signed = job_dir / "kiwi.apk"
        # The password reaches apksigner through its environment, never its arguments.
        self.builder.command(
            [java, "-jar", signer_jar, "sign", "--ks", store, "--ks-type", "PKCS12", "--ks-key-alias", ALIAS,
             "--ks-pass", "env:KIWI_KEYSTORE", "--key-pass", "env:KIWI_KEYSTORE",
             "--v1-signing-enabled", "false", "--v2-signing-enabled", "true", "--v3-signing-enabled", "true",
             "--v4-signing-enabled", "false", "--out", signed, source_path],
            env=sandbox.environment(job_dir, job_dir, KIWI_KEYSTORE=password), timeout=600,
            message="apksigner could not sign Kiwi's Android app; details are in the private log.")
        emit("stage", stage="Verifying the signed app")
        self.builder.command([java, "-jar", signer_jar, "verify", "--verbose", signed], timeout=600,
                             env=sandbox.environment(job_dir, job_dir),
                             message="apksigner does not accept the signed Kiwi app; details are in the private log.")
        try:
            result = apk.inspect(signed)
            unaligned = apk.misaligned(signed)
        except (apk.ApkError, OSError) as error:
            raise BuildError(f"The signed Kiwi app is not a valid package: {error}.") from error
        self.builder.log.write(f"kiwi: signed Kiwi app: {json.dumps(result)}\n".encode())
        if result["certificates"] != [signer]:
            raise BuildError("The signed Kiwi app is not signed with Kiwi's key.")
        if not {2, 3} <= set(result["schemes"]):
            raise BuildError("The signed Kiwi app lacks an APK signature v2 or v3.")
        if (result["package"], result["version_code"], result["version"]) != (
                KIWI_APP_PACKAGE, facts["version_code"], facts["version"]):
            raise BuildError("Signing changed Kiwi's Android app unexpectedly.")
        if unaligned:
            raise BuildError("The signed Kiwi app is not aligned (" + ", ".join(unaligned[:3]) + "). Use the APK "
                             "Gradle's assembleRelease makes, or align it with `zipalign -p 4` before placing it.")
        key = request.get("input")
        description = {
            "package": result["package"], "version_name": result["version"], "version_code": result["version_code"],
            "sha256": digest(signed), "size": signed.stat().st_size, "signer": signer, "source": source,
            "unsigned_sha256": unsigned_sha, "input": key if isinstance(key, str) and re.fullmatch(r"[0-9a-f]{64}", key) else None,
            "signed_at": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
        }
        self.publish_kiwi_app(signed, description)
        emit("kiwi_app", **{name: description[name] for name in
                            ("package", "version_name", "version_code", "sha256", "size", "source")})
        return description

    def publish_kiwi_app(self, signed, description):
        """Writes android-app/kiwi.apk and kiwi.json, each replaced in one step."""
        target = self.builder.root.parent / "android-app"
        target.mkdir(mode=0o700, exist_ok=True)
        apk_partial, meta_partial = target / "kiwi.apk.partial", target / "kiwi.json.partial"
        # A copy, not a rename: the job directory may be on another file system.
        shutil.copyfile(signed, apk_partial)
        meta_partial.write_text(json.dumps(description, indent=2, sort_keys=True) + "\n")
        for path in (apk_partial, meta_partial):
            with open(path, "rb") as handle:
                os.fsync(handle.fileno())
        # Kiwi offers the app only while kiwi.json describes kiwi.apk: removing the old
        # description first means it never sees the new file with the old description.
        (target / "kiwi.json").unlink(missing_ok=True)
        os.replace(apk_partial, target / "kiwi.apk")
        os.replace(meta_partial, target / "kiwi.json")


def run_kiwi_app(builder, android, request):
    """The `kiwi-app` command, with its private log and a work folder removed afterwards."""
    work = builder.tools / "jobs" / "kiwi-app"
    shutil.rmtree(work, ignore_errors=True)
    work.mkdir(mode=0o700, parents=True)
    log_path = builder.root / "kiwi-app.log"
    if log_path.is_file() and log_path.stat().st_size > 1024**2:
        log_path.replace(log_path.with_name("kiwi-app.log.1"))
    try:
        # Unbuffered: the tools Kiwi runs write to the same file.
        with open(log_path, "ab", buffering=0) as log:
            builder.log = log
            log.write(f"kiwi: kiwi-app at {time.strftime('%Y-%m-%dT%H:%M:%SZ', time.gmtime())}\n".encode())
            return android.kiwi_app(request, work)
    finally:
        shutil.rmtree(work, ignore_errors=True)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("command", choices=["plan", "build", "kiwi-app", "community"])
    parser.add_argument("--data-dir", required=True)
    parser.add_argument("--request")
    parser.add_argument("--job-id")
    parser.add_argument("--recipe", help="plan/build: the app recipe id")
    parser.add_argument("--supervisor-pid", type=int)
    args = parser.parse_args()
    os.umask(0o077)

    def interrupted(_sig, _frame):
        raise BuildError("Build cancelled")
    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGINT, interrupted)
    if args.supervisor_pid:
        def supervise():
            while True:
                time.sleep(2)
                if os.getppid() != args.supervisor_pid:
                    os.kill(os.getpid(), signal.SIGTERM)
                    return
        threading.Thread(target=supervise, daemon=True).start()
    builder = Builder(args.data_dir)
    android = Android(builder)
    request = json.loads(Path(args.request).read_text()) if args.request else {}
    if args.command == "kiwi-app":
        run_kiwi_app(builder, android, request if isinstance(request, dict) else {})
        return
    if args.command == "plan":
        android.plan(android.app(args.recipe or ""), request)
        return
    if not args.job_id or not re.fullmatch(r"[0-9a-f]{24}", args.job_id):
        raise BuildError("Invalid build job identifier")
    app = android.app(args.recipe or "") if args.command == "build" else None
    job_dir = builder.tools / "jobs" / args.job_id
    job_dir.mkdir(mode=0o700, exist_ok=True)
    log_dir = builder.root / "jobs" / args.job_id
    log_dir.mkdir(mode=0o700, exist_ok=True)
    # A process a tool detaches still comes back to this one, which ends it (sandbox.py).
    sandbox.adopt_orphans()
    try:
        with open(log_dir / "build.log", "ab") as log:
            builder.log = log
            if args.command == "community":
                # Community bundles: community.py, which requires the sandbox throughout.
                import community
                community.run(builder, request, job_dir)
            else:
                android.build(app, request["plan"], request["sequence"], job_dir)
    finally:
        sandbox.kill_strays()
        # Only the private log, the verified original cache and the artifact remain.
        for child in job_dir.iterdir():
            if child.is_dir() and not child.is_symlink():
                shutil.rmtree(child)
            else:
                child.unlink()


if __name__ == "__main__":
    try:
        require_python()
        main()
    except BuildError as error:
        kind = getattr(error, "kind", None)
        emit("error", message=str(error), blocked=error.blocked, **({"kind": kind} if kind else {}))
        sys.exit(1)
    except Exception:
        emit("error", message="The builder encountered an unexpected error; inspect the private build log.", blocked=False)
        sys.exit(1)
