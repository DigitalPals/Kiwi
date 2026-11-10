#!/usr/bin/env python3
"""Kiwi Builds for Android. Speaks the same JSON-lines protocol as runner.py.

A build fetches the original app at the version the recipe pins, proves the
publisher signed every byte of it (apk.py), patches it with the pinned patcher
and patch bundle, and signs the result with a key that never leaves this
computer. Nothing here needs an Android device, a Google account or macOS.

The original comes from the first source that can supply it:

1. `automation/android/originals/` - files the owner placed there by hand;
2. Google Play, through an anonymous session, at the pinned version code;
3. APKPure, at the pinned version.

A source is only a transport. Whatever it returns is accepted solely because
its APK signature verifies against the publisher certificate in the recipe.
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

RECIPES_PATH = Path(__file__).with_name("android.json")
PLAY = "https://android.clients.google.com/fdfe/"
PURE = "https://api.pureapk.com/m/v3/cms/app_version?hl=en-US&package_name="
MAX_APK = 1024**3
BUNDLES = (".apks", ".xapk", ".apkm")
ALIAS = "kiwi"
SESSION_SECONDS = 1800


class SourceError(Exception):
    """One source could not supply a trustworthy original; the next is tried."""


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
            download(link, path, limit=size + 1024 if isinstance(size, int) and 0 < size < MAX_APK else MAX_APK)
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

    # --- signing key -------------------------------------------------------

    def keystore(self, java):
        """Kiwi's Android signing key: made once, kept beside the other private state.

        Android only installs an update over an app signed with the same key, so
        this key is the identity of every app Kiwi builds for this owner.
        """
        store, secret = self.private / "keystore.p12", self.private / "keystore.pass"
        keytool = java.with_name("keytool")
        if not store.is_file():
            emit("stage", stage="Creating Kiwi's Android signing key")
            password = secrets.token_hex(24)
            partial = store.with_name("keystore.partial")
            partial.unlink(missing_ok=True)
            self.builder.command([keytool, "-genkeypair", "-keystore", partial, "-storetype", "PKCS12", "-alias", ALIAS,
                                  "-keyalg", "RSA", "-keysize", "4096", "-sigalg", "SHA256withRSA", "-validity", "36500",
                                  "-dname", "CN=Kiwi", "-storepass:env", "KIWI_KEYSTORE", "-keypass:env", "KIWI_KEYSTORE"],
                                 env=dict(self.builder.env, KIWI_KEYSTORE=password),
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
                                    "-storepass:env", "KIWI_KEYSTORE"], env=dict(self.builder.env, KIWI_KEYSTORE=password),
                                   capture=True, message="Kiwi's Android signing key could not be read.")
        body = re.search(r"-----BEGIN CERTIFICATE-----(.+?)-----END CERTIFICATE-----", pem, re.S)
        if not body:
            raise BuildError("Kiwi's Android signing key could not be read.")
        return store, password, hashlib.sha256(base64.b64decode(body.group(1))).hexdigest()

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
                "version": app["app_version"], "recipe": {"toolchain": self.toolchain, "app": app},
                "tweaks": self.options(app, options), "name": None, "runner_sha256": self.sources_digest()}
        plan["key"] = recipe_key(plan)
        emit("plan", plan=plan)

    def build(self, app, plan, sequence, job_dir):
        expected_key = plan["key"]
        if (expected_key != recipe_key({k: v for k, v in plan.items() if k != "key"}) or plan.get("app") != app["id"]
                or plan["recipe"] != {"toolchain": self.toolchain, "app": app} or plan["runner_sha256"] != self.sources_digest()):
            raise BuildError("The build plan changed; check for updates again")
        if not isinstance(sequence, int) or not 0 < sequence < 2**53:
            raise BuildError("Invalid Kiwi build revision")
        tweaks = self.options(app, plan)
        java = self.java()
        patches = self.tool("patches")
        patcher = self.tool("patcher")
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
        for name in ("patcher", "home"):
            (job_dir / name).mkdir(mode=0o700)
        # Patches the recipe leaves out, such as the one that replaces the app's own icon.
        selection = [arg for name in app.get("disabled_patches", []) for arg in ("--disable", name)]
        for package in app["packages"]:
            for name in package.get("patches", []) if package.get("optional") else []:
                selection += ["--enable" if package["id"] in tweaks else "--disable", name]
        candidate, result = job_dir / "candidate.apk", job_dir / "result.json"
        self.builder.command(
            [java, "-jar", patcher, "patch", "--patches", patches, "--out", candidate,
             "--temporary-files-path", job_dir / "work", "--result-file", result,
             "--keystore", store, "--keystore-password", password, "--keystore-entry-alias", ALIAS,
             "--keystore-entry-password", password, "--signer", "Kiwi", "--striplibs", self.toolchain["abi"],
             *selection, original], cwd=job_dir, timeout=3600,
            # The patcher records every app it patches; keep that inside the job.
            env=dict(self.builder.env, MORPHE_DATA_DIR=str(job_dir / "patcher"), HOME=str(job_dir / "home")),
            message=f"Patching failed. {app['name']} {plan['version']} may need an updated patch recipe.")
        try:
            outcome = json.loads(result.read_text())
        except (OSError, ValueError) as error:
            raise BuildError("The patcher reported no result") from error
        if outcome.get("success") is not True or outcome.get("failedPatches") or not outcome.get("appliedPatches"):
            raise BuildError(f"Some patches did not apply to {app['name']} {plan['version']}; nothing was published")
        emit("stage", stage="Validating the patched app")
        try:
            patched = apk.inspect(candidate)
        except (apk.ApkError, OSError) as error:
            raise BuildError("The patched app is not a valid signed package") from error
        self.builder.log.write(f"kiwi: patched app: {json.dumps(patched)}\n".encode())
        if patched["certificates"] != [signer]:
            raise BuildError("The patched app is not signed with Kiwi's key")
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


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("command", choices=["plan", "build"])
    parser.add_argument("--data-dir", required=True)
    parser.add_argument("--request")
    parser.add_argument("--job-id")
    parser.add_argument("--recipe", required=True)
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
    app = android.app(args.recipe)
    if args.command == "plan":
        android.plan(app, request)
        return
    if not args.job_id or not re.fullmatch(r"[0-9a-f]{24}", args.job_id):
        raise BuildError("Invalid build job identifier")
    job_dir = builder.tools / "jobs" / args.job_id
    job_dir.mkdir(mode=0o700, exist_ok=True)
    log_dir = builder.root / "jobs" / args.job_id
    log_dir.mkdir(mode=0o700, exist_ok=True)
    try:
        with open(log_dir / "build.log", "ab") as log:
            builder.log = log
            android.build(app, request["plan"], request["sequence"], job_dir)
    finally:
        # Only the private log, the verified original cache and the artifact remain.
        for child in job_dir.iterdir():
            if child.is_dir() and not child.is_symlink():
                shutil.rmtree(child)
            else:
                child.unlink()


if __name__ == "__main__":
    try:
        main()
    except BuildError as error:
        emit("error", message=str(error), blocked=error.blocked)
        sys.exit(1)
    except Exception:
        emit("error", message="The builder encountered an unexpected error; inspect the private build log.", blocked=False)
        sys.exit(1)
