#!/usr/bin/env python3
"""Kiwi's private macOS app builder. Stdout is a bounded JSON-lines protocol.

Tool output and account state never enter the HTTP API. Dependencies live under
Kiwi's automation/tools directory; recipes and per-build provenance pin sources.
Each pinned source is fetched once into tools/sources and reused from there.
"""
import argparse
import contextlib
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import plistlib
import re
import shutil
import signal
import stat
import struct
import subprocess
import sys
import tempfile
import threading
import time
import zipfile

MAX_IPA = 4 * 1024**3
MAX_EXPANDED = 8 * 1024**3
RECIPES_PATH = Path(__file__).with_name("recipes.json")
TOOL_FORMAT = 3
# Extensions are removed during injection, so they need not be decrypted first.
REMOVED_PARTS = ("PlugIns", "Extensions", "Watch")
REPO = re.compile(r"[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+")


class BuildError(Exception):
    def __init__(self, message, blocked=False):
        super().__init__(message)
        self.blocked = blocked


def emit(kind, **data):
    print(json.dumps({"event": kind, **data}, sort_keys=True), flush=True)


def digest(path):
    with open(path, "rb") as f:
        return hashlib.file_digest(f, "sha256").hexdigest()


def tree_digest(path):
    """SHA-256 of a file, or of a directory's relative paths and file digests."""
    if not path.is_dir():
        return digest(path)
    entries = sorted(p for p in path.rglob("*") if p.is_file() and not p.is_symlink())
    return hashlib.sha256(canonical([[p.relative_to(path).as_posix(), digest(p)] for p in entries])).hexdigest()


def canonical(value):
    return json.dumps(value, sort_keys=True, separators=(",", ":")).encode()


def recipe_key(plan):
    return hashlib.sha256(canonical(plan)).hexdigest()


def read_json_output(raw):
    """ipatool logs JSON records; select its last successful result."""
    records = []
    for line in raw.splitlines():
        try:
            value = json.loads(line)
            if isinstance(value, dict):
                records.append(value)
        except ValueError:
            pass
    for record in reversed(records):
        if record.get("success") is True:
            return record
    raise BuildError("The App Store tool returned no successful result. Sign in again on the Mac.", True)


def run(argv, *, cwd=None, env=None, timeout=900, log=None, capture=False, message=None, umask=-1):
    """Never use a shell, never expose raw Apple responses, reap the whole tool group."""
    output = tempfile.TemporaryFile() if capture else log
    if output is None:
        output = subprocess.DEVNULL
    process = subprocess.Popen([str(x) for x in argv], cwd=cwd, env=env,
                               stdin=subprocess.DEVNULL, stdout=output,
                               stderr=log or subprocess.DEVNULL, start_new_session=True, umask=umask)
    def terminate(_sig=None, _frame=None):
        with contextlib.suppress(ProcessLookupError):
            os.killpg(process.pid, signal.SIGKILL)
        process.wait()
        raise BuildError("Build cancelled")
    previous = {s: signal.signal(s, terminate) for s in (signal.SIGTERM, signal.SIGINT)}
    try:
        try:
            status = process.wait(timeout=timeout)
        except subprocess.TimeoutExpired:
            terminate()
        if status:
            raise BuildError(message or f"{Path(str(argv[0])).name} failed; details are in the private Mac build log")
        if capture:
            output.seek(0, 2)
            if output.tell() > 4 * 1024**2:
                raise BuildError("Tool output exceeded its limit")
            output.seek(0)
            return output.read().decode("utf-8", "replace")
        return ""
    finally:
        if process.poll() is None:
            with contextlib.suppress(ProcessLookupError):
                os.killpg(process.pid, signal.SIGKILL)
            process.wait()
        for s, handler in previous.items():
            signal.signal(s, handler)
        if capture:
            output.close()


def macho_info(data):
    """Validate encryption commands in thin/FAT Mach-O headers (including extensions)."""
    if len(data) < 4:
        return None
    magic = data[:4]
    if magic in (b"\xca\xfe\xba\xbe", b"\xca\xfe\xba\xbf"):
        if len(data) < 8:
            raise BuildError("Truncated FAT executable")
        count = struct.unpack_from(">I", data, 4)[0]
        stride = 32 if magic[-1] == 0xbf else 20
        if count > 32 or 8 + count * stride > len(data):
            raise BuildError("Invalid FAT executable")
        encrypted, libraries = False, []
        for i in range(count):
            pos = 8 + i * stride + 8
            offset = struct.unpack_from(">Q" if stride == 32 else ">I", data, pos)[0]
            info = macho_info(data[offset:])
            if info is None:
                raise BuildError("Missing FAT executable slice")
            encrypted |= info[0]
            libraries.extend(info[1])
        return encrypted, libraries
    formats = {b"\xcf\xfa\xed\xfe": ("<", 32), b"\xce\xfa\xed\xfe": ("<", 28),
               b"\xfe\xed\xfa\xcf": (">", 32), b"\xfe\xed\xfa\xce": (">", 28)}
    if magic not in formats:
        return None
    endian, pos = formats[magic]
    if len(data) < pos:
        raise BuildError("Truncated Mach-O header")
    commands, size = struct.unpack_from(endian + "II", data, 16)
    end = pos + size
    if commands > 65536 or end > len(data):
        raise BuildError("Invalid Mach-O load commands")
    encrypted, libraries = False, []
    for _ in range(commands):
        if pos + 8 > end:
            raise BuildError("Truncated Mach-O command")
        cmd, length = struct.unpack_from(endian + "II", data, pos)
        if length < 8 or pos + length > end:
            raise BuildError("Invalid Mach-O command length")
        if cmd in (0x21, 0x2c):
            if length < 20:
                raise BuildError("Invalid encryption command")
            encrypted |= struct.unpack_from(endian + "I", data, pos + 16)[0] != 0
        if cmd in (0xc, 0x80000018, 0x8000001f):
            if length < 24:
                raise BuildError("Invalid dylib command")
            offset = struct.unpack_from(endian + "I", data, pos + 8)[0]
            if offset < 24 or offset >= length:
                raise BuildError("Invalid dylib name")
            libraries.append(data[pos + offset:pos + length].split(b"\0", 1)[0].decode("utf-8", "replace"))
        pos += length
    return encrypted, libraries


def inspect_ipa(path, bundle_id, *, decrypted=False, markers=(), removable=False):
    """Validate an IPA of `bundle_id`. `markers` must name injected libraries the main
    executable loads; `removable` skips extensions that injection will remove."""
    if path.stat().st_size > MAX_IPA:
        raise BuildError("IPA exceeds the 4 GiB limit")
    with zipfile.ZipFile(path) as archive:
        entries = archive.infolist()
        if len(entries) > 100000 or sum(e.file_size for e in entries) > MAX_EXPANDED:
            raise BuildError("IPA expanded size exceeds its limit")
        seen = set()
        for e in entries:
            p = PurePosixPath(e.filename)
            if p.is_absolute() or ".." in p.parts or "\\" in e.filename or e.filename in seen:
                raise BuildError("IPA contains unsafe or duplicate paths")
            seen.add(e.filename)
            if stat.S_ISLNK(e.external_attr >> 16):
                target = archive.read(e).decode("utf-8")
                if target.startswith("/") or ".." in PurePosixPath(target).parts:
                    raise BuildError("IPA contains an unsafe symbolic link")
        mains = [e for e in entries if re.fullmatch(r"Payload/[^/]+\.app/Info\.plist", e.filename)]
        if len(mains) != 1 or mains[0].file_size > 4 * 1024**2:
            raise BuildError("IPA must contain one app with a bounded Info.plist")
        info = plistlib.loads(archive.read(mains[0]))
        if info.get("CFBundleIdentifier") != bundle_id:
            raise BuildError("The downloaded app has an unexpected bundle identifier")
        root = mains[0].filename.rsplit("/", 1)[0]
        executable = info.get("CFBundleExecutable", "")
        if not executable or "/" in executable or executable in (".", ".."):
            raise BuildError("Invalid app executable")
        main = root + "/" + executable
        main_info = None
        if decrypted:
            for e in entries:
                if e.is_dir() or e.file_size < 4 or stat.S_ISLNK(e.external_attr >> 16):
                    continue
                inside = PurePosixPath(e.filename).relative_to(root).parts if e.filename.startswith(root + "/") else ()
                if removable and inside and inside[0] in REMOVED_PARTS:
                    continue
                with archive.open(e) as f:
                    magic = f.read(4)
                if magic not in (b"\xcf\xfa\xed\xfe", b"\xce\xfa\xed\xfe", b"\xfe\xed\xfa\xcf", b"\xfe\xed\xfa\xce", b"\xca\xfe\xba\xbe", b"\xca\xfe\xba\xbf"):
                    continue
                if e.file_size > 1024**3:
                    raise BuildError("Executable is too large")
                result = macho_info(archive.read(e))
                if result[0]:
                    raise BuildError("An executable remains encrypted; the IPA was not published")
                if e.filename == main:
                    main_info = result
            if main_info is None:
                raise BuildError("The main executable is missing or invalid")
        libraries = main_info[1] if main_info else []
        for marker in markers:
            if not any(marker in n.lower() for n in seen) or not any(marker in lib.lower() for lib in libraries):
                raise BuildError(f"The {marker} patch is missing from the app or its executable load commands")
        return info


def stamp_revision(source, target, revision):
    # YouTube reads CFBundleVersion in YTVersionUtils.appVersion. Replacing it
    # changes its API client version and can prevent all content from loading.
    # Keep both Apple version fields intact and track our packaging separately.
    if not isinstance(revision, int) or not 0 < revision < 2**53:
        raise BuildError("Invalid Kiwi build revision")
    with zipfile.ZipFile(source) as src, zipfile.ZipFile(target, "w") as dst:
        for e in src.infolist():
            if re.fullmatch(r"Payload/[^/]+\.app/Info\.plist", e.filename):
                info = plistlib.loads(src.read(e))
                info["KiwiBuildRevision"] = revision
                dst.writestr(e, plistlib.dumps(info))
            else:
                with src.open(e) as r, dst.open(e, "w") as w:
                    shutil.copyfileobj(r, w, 1024 * 1024)


def apply_patches(source, patches, name):
    """Exact, single-occurrence edits to a job's copy of a pinned source."""
    for patch in patches:
        path = (source / patch["file"]).resolve()
        if source.resolve() not in path.parents:
            raise BuildError("Recipe patches must stay inside their source")
        text = path.read_text()
        if text.count(patch["find"]) != 1:
            raise BuildError(f"The pinned {name} source no longer matches Kiwi's build patch")
        path.write_text(text.replace(patch["find"], patch["replace"]))


def installed_ipa(app, target):
    """Use the original encrypted app and license already installed by Apple.

    This avoids a second download just for enrollment. UnfairPlay validates the
    license selector and performs a complete sample decryption during setup.
    """
    infos = list(app.glob("Wrapper/*.app/Info.plist")) or list(app.glob("Info.plist"))
    if len(infos) != 1:
        raise BuildError("Select an iPhone/iPad app installed from the Mac App Store")
    native = infos[0].parent
    with zipfile.ZipFile(target, "w", zipfile.ZIP_DEFLATED) as archive:
        for base, dirs, files in os.walk(native):
            for name in dirs + files:
                path = Path(base) / name
                member = "Payload/" + native.name + "/" + path.relative_to(native).as_posix()
                if path.is_symlink():
                    entry = zipfile.ZipInfo(member)
                    entry.external_attr = 0o120777 << 16
                    archive.writestr(entry, os.readlink(path))
                else:
                    archive.write(path, member)


class Builder:
    def __init__(self, data):
        self.root = Path(data).resolve() / "automation"
        self.root.mkdir(mode=0o700, parents=True, exist_ok=True)
        # Theos/make do not support spaces in their source/toolchain paths. Keep
        # the private data in Application Support and build beneath ~/.kiwi-builder-tools
        # (an existing link, such as ~/Code/Kiwi-builder-tools, is kept).
        tools_link = self.root / "tools"
        if not tools_link.is_symlink() and " " in str(tools_link):
            destination = Path.home() / ".kiwi-builder-tools"
            if tools_link.exists():
                if destination.exists():
                    raise BuildError("The build tools destination already exists; inspect it before migrating")
                shutil.move(str(tools_link), destination)
            else:
                destination.mkdir(mode=0o700, parents=True, exist_ok=True)
            tools_link.symlink_to(destination, target_is_directory=True)
        self.tools = tools_link.resolve()
        self.recipes = json.loads(RECIPES_PATH.read_text())
        self.toolchain = self.recipes["toolchain"]
        self.log = None
        # Setup installs missing tools; it must not upgrade ones already installed.
        self.env = dict(os.environ, GIT_TERMINAL_PROMPT="0", HOMEBREW_NO_AUTO_UPDATE="1", HOMEBREW_NO_INSTALL_UPGRADE="1",
                        PATH=f"{self.tools}/bin:{self.tools}/venv/bin:/opt/homebrew/opt/make/libexec/gnubin:/opt/homebrew/bin:/usr/bin:/bin:/usr/sbin:/sbin")
        for p in (self.root, self.tools, self.tools / "bin", self.tools / "jobs", self.root / "models", self.root / "artifacts", self.root / "jobs"):
            p.mkdir(mode=0o700, parents=True, exist_ok=True)

    def command(self, args, env=None, **kwargs):
        return run(args, env=env or self.env, log=self.log, **kwargs)

    def app(self, app_id):
        for app in self.recipes["apps"]:
            if app["id"] == app_id:
                return app
        raise BuildError("Unknown app recipe")

    def tool(self, name):
        found = shutil.which(name, path=self.env["PATH"])
        if not found:
            raise BuildError("Build tools need installing on the Mac. Run the documented setup command.", True)
        return found

    def ipa(self, args):
        try:
            raw = self.command([self.tool("ipatool"), "--format", "json", "--non-interactive", *args], capture=True,
                               timeout=1800 if args[0] == "download" else 300,
                               message="App Store access failed. Sign in with Kiwi’s ipatool on the Mac, then retry.")
            return read_json_output(raw)
        except BuildError as e:
            raise BuildError(str(e), True) from e

    def checkout(self, spec, target, recursive=False):
        repo, revision = spec[:2]
        if not REPO.fullmatch(repo) or not re.fullmatch(r"[0-9a-f]{40}", revision):
            raise BuildError("Recipe revisions must be complete Git commit hashes")
        target.parent.mkdir(parents=True, exist_ok=True)
        if not (target / ".git").is_dir():
            self.command(["git", "init", "--quiet", target])
            self.command(["git", "-C", target, "remote", "add", "origin", f"https://github.com/{repo}.git"])
        self.command(["git", "-C", target, "fetch", "--quiet", "--depth=1", "origin", revision])
        self.command(["git", "-C", target, "checkout", "--quiet", "--detach", "FETCH_HEAD"])
        if recursive:
            self.command(["git", "-C", target, "submodule", "update", "--init", "--recursive", "--depth=1"], timeout=1800)
        return target

    def source(self, spec, submodules=False):
        """The pinned tree in tools/sources, fetched from GitHub only the first time."""
        repo, revision = spec[:2]
        if not REPO.fullmatch(repo) or not re.fullmatch(r"[0-9a-f]{40}", revision):
            raise BuildError("Recipe revisions must be complete Git commit hashes")
        tree = self.tools / "sources" / repo.replace("/", "__") / (revision + ("+modules" if submodules else ""))
        complete = tree.with_name(tree.name + ".complete")
        if complete.is_file():
            return tree
        emit("stage", stage=f"Fetching pinned {repo.split('/')[1]} source")
        partial = tree.with_name(tree.name + ".partial")
        for stale in (tree, partial):
            if stale.exists():
                shutil.rmtree(stale)
        self.checkout(spec, partial, recursive=submodules)
        partial.rename(tree)
        complete.write_text(revision)
        return tree

    def materialize(self, spec, target, submodules=False):
        """A private copy of a cached source for one job (APFS clones it for free)."""
        cached = self.source(spec, submodules)
        if target.exists():
            shutil.rmtree(target)
        target.parent.mkdir(parents=True, exist_ok=True)
        if sys.platform == "darwin":
            self.command(["cp", "-cR", cached, target])
        else:
            shutil.copytree(cached, target, symlinks=True)
        if self.command(["git", "-C", target, "rev-parse", "HEAD"], capture=True).strip() != spec[1]:
            raise BuildError("A cached source does not match its pinned commit; delete it and retry")
        return target

    def app_sources(self, app):
        """Every pinned source an app's build may need, as (spec, submodules)."""
        for package in app["packages"]:
            yield package["repo"], package.get("submodules", False)
            for sibling in package.get("siblings", []):
                yield sibling, False
            prebuild = package.get("prebuild")
            if prebuild:
                yield prebuild["openssl"], False
                yield prebuild["ffmpeg"], False

    def doctor(self):
        checks = []
        checks.append({"name": "Mac build tools", "ready": sys.platform == "darwin" and all(shutil.which(t, path=self.env["PATH"]) for t in ("ipatool", "unfairplay", "cyan", "gmake", "ldid", "dpkg-deb"))})
        try:
            setup = json.loads((self.tools / "setup.json").read_text())
        except (OSError, ValueError):
            setup = {}
        checks.append({"name": "Tweak toolchain", "ready": (self.tools / "theos/makefiles/common.mk").is_file()
                       and setup.get("recipe") == recipe_key(self.toolchain) and setup.get("format") == TOOL_FORMAT})
        try:
            self.ipa(["auth", "info"])
            authenticated = True
        except BuildError:
            authenticated = False
        checks.append({"name": "App Store sign-in", "ready": authenticated})
        models = list((self.root / "models").glob("*.bin"))
        checks.append({"name": "Private decryption enrollment", "ready": bool(models) and all(p.stat().st_mode & 0o077 == 0 for p in models)})
        return {"checks": checks, "ready": all(c["ready"] for c in checks)}

    def setup(self):
        if sys.platform != "darwin":
            raise BuildError("Setup must run on macOS")
        self.command(["/opt/homebrew/bin/brew", "install", "go", "make", "ldid", "dpkg", "python@3.13"], timeout=3600)
        repositories = self.toolchain["repositories"]
        for name, package in (("ipatool", "."), ("unfairplay", "./cmd/unfairplay")):
            source = self.checkout(repositories[name], self.tools / "src" / name)
            if name == "ipatool":
                # The pinned library returns Apple's current build identifier,
                # but its CLI omits it. Expose that existing metadata explicitly;
                # version-history order is not a reliable latest-version signal.
                command = source / "cmd/list_versions.go"
                text = command.read_text()
                before = 'Interface("externalVersionIdentifiers", out.ExternalVersionIdentifiers).'
                after = before + '\n\t\t\t\t\tStr("latestExternalVersionID", out.LatestExternalVersionID).'
                if after not in text:
                    if text.count(before) != 1:
                        raise BuildError("The pinned ipatool metadata adapter no longer matches")
                    command.write_text(text.replace(before, after))
            self.command(["go", "build", "-o", self.tools / "bin" / name, package], cwd=source, timeout=1800)
        cyan = self.checkout(repositories["cyan"], self.tools / "src" / "cyan")
        self.command(["/opt/homebrew/bin/python3.13", "-m", "venv", self.tools / "venv"])
        self.command([self.tools / "venv/bin/pip", "install", str(cyan)], timeout=1800)
        theos = self.checkout(repositories["theos"], self.tools / "theos", recursive=True)
        self.command(["git", "-C", theos / "vendor/logos", "fetch", "--quiet", "origin", self.toolchain["logos_revision"]])
        self.command(["git", "-C", theos / "vendor/logos", "checkout", "--quiet", "--detach", "FETCH_HEAD"])
        for subdir, spec in self.toolchain["headers"].items():
            self.checkout(spec, theos / "include" / subdir)
        sdk = self.command(["xcrun", "--sdk", "iphoneos", "--show-sdk-path"], capture=True).strip()
        sdk_version = self.command(["xcrun", "--sdk", "iphoneos", "--show-sdk-version"], capture=True).strip()
        (theos / "sdks").mkdir(exist_ok=True)
        link = theos / "sdks" / f"iPhoneOS{sdk_version}.sdk"
        if not link.exists():
            link.symlink_to(sdk)
        # Fetch every app's pinned sources now, so builds do not need GitHub.
        for app in self.recipes["apps"]:
            for spec, submodules in self.app_sources(app):
                self.source(spec, submodules)
        (self.tools / "setup.json").write_bytes(canonical({"recipe": recipe_key(self.toolchain), "sdk": sdk_version, "format": TOOL_FORMAT}))
        emit("result", setup=True)

    def version_of(self, app, external):
        metadata = self.ipa(["get-version-metadata", "--app-id", app["app_id"], "--external-version-id", external, "--platform", "iphone"])
        version = metadata.get("displayVersion", "")
        if not re.fullmatch(r"\d+(?:\.\d+){0,3}", version):
            raise BuildError(f"The App Store returned an invalid {app['name']} version")
        return version

    def pinned_build(self, app, ids, latest):
        """Apple's build of the version the app's tweak supports, newest first. Versions of
        build ids never change, so lookups are remembered."""
        target, cache_path = app["app_version"], self.tools / "versions" / f"{app['app_id']}.json"
        try:
            known = json.loads(cache_path.read_text())
        except (OSError, ValueError):
            known = {}
        candidates = [latest] + [x for x in sorted(ids, key=int, reverse=True) if x != latest]
        try:
            for external in candidates[:40]:
                if external not in known:
                    known[external] = self.version_of(app, external)
                if known[external] == target:
                    return external
        finally:
            cache_path.parent.mkdir(mode=0o700, exist_ok=True)
            cache_path.write_text(json.dumps(known, sort_keys=True))
        raise BuildError(f"The App Store no longer offers {app['name']} {target}. Kiwi needs a newer {app['tweak']} recipe.", True)

    def options(self, app, options):
        optional = [p["id"] for p in app["packages"] if p.get("optional")]
        tweaks = sorted(set(options.get("tweaks", [])))
        if any(t not in optional for t in tweaks):
            raise BuildError("Unknown tweak selection")
        name = options.get("name")
        if app.get("names") and name not in app["names"]:
            raise BuildError("Unknown app name")
        return tweaks, name if app.get("names") else None

    def plan(self, app, options):
        health = self.doctor()
        emit("health", **health)
        if not health["ready"]:
            missing = ", ".join(c["name"] for c in health["checks"] if not c["ready"])
            raise BuildError("Setup required on the Mac: " + missing, True)
        tweaks, name = self.options(app, options)
        emit("stage", stage="Checking the App Store")
        try:
            versions = self.ipa(["list-versions", "--app-id", app["app_id"], "--platform", "iphone"])
        except BuildError:
            # A new account may not yet own the free app's license.
            self.ipa(["purchase", "--bundle-identifier", app["bundle_id"], "--platform", "iphone"])
            versions = self.ipa(["list-versions", "--app-id", app["app_id"], "--platform", "iphone"])
        ids = [str(x) for x in versions.get("externalVersionIdentifiers", [])]
        if not ids or not all(x.isdigit() for x in ids):
            raise BuildError(f"The App Store returned no valid {app['name']} build identifiers")
        external = str(versions.get("latestExternalVersionID", ""))
        if external not in ids:
            raise BuildError(f"The App Store returned no current {app['name']} build identifier")
        if app.get("app_version"):
            external, version = self.pinned_build(app, ids, external), app["app_version"]
        else:
            version = self.version_of(app, external)
        plan = {"app": app["id"], "external_id": external, "version": version,
                "recipe": {"toolchain": self.toolchain, "app": app}, "tweaks": tweaks, "name": name,
                "runner_sha256": digest(Path(__file__)),
                "toolchain": json.loads((self.tools / "setup.json").read_text())}
        plan["key"] = recipe_key(plan)
        emit("plan", plan=plan)

    def prebuild_ffmpeg_kit(self, source, spec, job_dir):
        """Build NeoFreeBird's FFmpeg stack from pinned OpenSSL and FFmpeg sources instead of
        the script's own downloads, and reuse the result while its inputs are unchanged."""
        script = source / spec["script"]
        text = script.read_text()
        tag = spec["ffmpeg_tag"]
        if f"FFMPEG_TAG={tag}\n" not in text:
            raise BuildError("The tweak's FFmpeg version changed; Kiwi's recipe needs updating")
        kit = self.command(["git", "-C", source / spec["kit"], "rev-parse", "HEAD"], capture=True).strip()
        setup = json.loads((self.tools / "setup.json").read_text())
        key = recipe_key({"spec": spec, "script": hashlib.sha256(text.encode()).hexdigest(), "kit": kit, "sdk": setup.get("sdk")})
        cache = self.tools / "cache" / "ffmpeg-kit" / key
        complete = cache.with_name(key + ".complete")
        if not complete.is_file():
            emit("stage", stage="Building FFmpeg from pinned sources")
            build = job_dir / "ffmpeg-build"
            build.mkdir()
            self.materialize(spec["openssl"], build / "openssl")
            ffmpeg = self.source(spec["ffmpeg"])
            self.command(["git", "-C", ffmpeg, "archive", "--format=tar.gz", f"--prefix=FFmpeg-{tag}/", "-o", build / "ffmpeg.tar.gz", "HEAD"])
            self.command([script], cwd=source, env=dict(self.env, BUILD_DIR=str(build)), timeout=3600, umask=0o022,
                         message="FFmpeg did not compile from its pinned sources.")
            if cache.exists():
                shutil.rmtree(cache)
            shutil.copytree(source / spec["output"], cache, symlinks=True)
            complete.write_text(key)
        output = source / spec["output"]
        if output.exists():
            shutil.rmtree(output)
        shutil.copytree(cache, output, symlinks=True)

    def compile_tweaks(self, app, tweaks, job_dir):
        emit("stage", stage=f"Compiling {app['tweak'].split(' ')[0]} and selected tweaks")
        self.env["THEOS"] = str(self.tools / "theos")
        selected = [p for p in app["packages"] if not p.get("optional") or p["id"] in tweaks]
        if any(t not in [p["id"] for p in app["packages"] if p.get("optional")] for t in tweaks):
            raise BuildError("Unknown tweak selection")
        products = []
        for package in selected:
            name = package["id"]
            # Several tweaks include headers from sibling repositories by their
            # upstream names (for example ../YTVideoOverlay/Header.h).
            sources = job_dir / "sources"
            source = sources / package["repo"][0].rsplit("/", 1)[1]
            if not source.exists():
                self.materialize(package["repo"], source, package.get("submodules", False))
            for sibling in package.get("siblings", []):
                if not (sources / sibling[0].rsplit("/", 1)[1]).exists():
                    self.materialize(sibling, sources / sibling[0].rsplit("/", 1)[1])
            apply_patches(source, package.get("patches", []), name)
            # Public source files are packaged inside a private workspace. Git
            # inherits our private umask, but Debian requires package control
            # directories/scripts to have ordinary distribution permissions.
            for base, dirs, files in os.walk(source):
                dirs[:] = [d for d in dirs if d != ".git"]
                for entry in dirs + files:
                    path = Path(base) / entry
                    if not path.is_symlink():
                        mode = path.stat().st_mode
                        path.chmod(0o755 if path.is_dir() or mode & 0o111 else 0o644)
            if package.get("prebuild", {}).get("kind") == "ffmpeg-kit":
                self.prebuild_ffmpeg_kit(source, package["prebuild"], job_dir)
            target = package.get("target", "package")
            self.command([self.tool("gmake"), "clean", target, "DEBUG=0", "FINALPACKAGE=1", "THEOS_PACKAGE_SCHEME=rootless", "ARCHS=arm64",
                          *package.get("make", [])], cwd=source, timeout=package.get("timeout", 1200),
                         message=f"The {name} tweak did not compile. Its pinned source may need a compatibility fix.", umask=0o022)
            if "products" in package:
                # Built files cyan injects directly, as the tweak's own sideload build does.
                built = [source / p for p in package["products"]]
                if not all(p.exists() and source.resolve() in p.resolve().parents for p in built):
                    raise BuildError(f"The {name} tweak did not produce its expected files")
            else:
                built = list((source / "packages").glob("*.deb"))
                if len(built) != 1:
                    raise BuildError(f"Expected one compiled package for {name}")
            products.extend(built)
        return products

    def build(self, app, plan, sequence, job_dir):
        expected_key = plan["key"]
        if (expected_key != recipe_key({k: v for k, v in plan.items() if k != "key"}) or plan.get("app") != app["id"]
                or plan["recipe"] != {"toolchain": self.toolchain, "app": app} or plan["runner_sha256"] != digest(Path(__file__))):
            raise BuildError("The build plan changed; check for updates again")
        tweaks, name = self.options(app, plan)
        original = job_dir / "original.ipa"
        emit("stage", stage=f"Downloading original {app['name']}")
        self.ipa(["download", "--app-id", app["app_id"], "--external-version-id", plan["external_id"], "--platform", "iphone", "--purchase", "--output", str(original)])
        info = inspect_ipa(original, app["bundle_id"])
        if info.get("CFBundleShortVersionString") != plan["version"]:
            raise BuildError(f"Apple served a different {app['name']} version than requested")
        original_sha = digest(original)
        emit("stage", stage=f"Decrypting {app['name']}")
        decrypted = job_dir / "decrypted.ipa"
        self.command([self.tool("unfairplay"), "decrypt", "--input", original, "--output", decrypted, "--model", self.root / "models"], timeout=1800,
                     message="Decryption failed. Verify the Mac enrollment matches the App Store account.")
        original.unlink()
        inspect_ipa(decrypted, app["bundle_id"], decrypted=True, removable=True)
        products = self.compile_tweaks(app, tweaks, job_dir)
        emit("stage", stage="Injecting tweaks")
        injected = job_dir / "injected.ipa"
        display = name or app.get("display_name")
        self.command([self.tool("cyan"), "-i", decrypted, "-o", injected, "-uwef", *products,
                      *(["-n", display] if display else []), "-b", app["bundle_id"]], timeout=1800,
                     message=f"Tweak injection failed. The new {app['name']} build may need an updated patch recipe.")
        candidate = job_dir / "candidate.ipa"
        stamp_revision(injected, candidate, sequence)
        emit("stage", stage="Validating patched executables")
        patched = inspect_ipa(candidate, app["bundle_id"], decrypted=True, markers=app["markers"])
        for key in ("CFBundleVersion", "CFBundleShortVersionString"):
            if not info.get(key) or patched.get(key) != info[key]:
                raise BuildError(f"Patching changed {app['name']}’s original version metadata")
        sha = digest(candidate)
        artifact = self.root / "artifacts" / f"{sha}.ipa"
        os.replace(candidate, artifact)
        provenance = {"plan": plan, "sequence": sequence, "base_build": info.get("CFBundleVersion"),
                      "original_sha256": original_sha, "sha256": sha, "built_at": time.time(),
                      "packages": [{"name": p.name, "sha256": tree_digest(p)} for p in products],
                      "runtime_tested": False}
        (self.root / "artifacts" / f"{sha}.json").write_bytes(canonical(provenance))
        emit("artifact", sha256=sha, version=patched["CFBundleShortVersionString"], build=patched["CFBundleVersion"], revision=sequence,
             min_os=patched.get("MinimumOSVersion"), size=artifact.stat().st_size,
             notes=f"{app['name']} {plan['version']} · Apple build {plan['external_id']}\n{app['tweak']} and tweaks compiled from pinned sources.\nRecipe {expected_key[:12]}. Not yet tested on a physical device.")


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("command", choices=["doctor", "setup", "plan", "build", "compile", "enroll"])
    parser.add_argument("--data-dir", required=True)
    parser.add_argument("--request")
    parser.add_argument("--job-id")
    parser.add_argument("--app", help="enroll: the installed Mac App Store app")
    parser.add_argument("--recipe", default="youtube", help="plan/build/compile: the app recipe id")
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
    request = json.loads(Path(args.request).read_text()) if args.request else {}
    if args.command == "setup":
        with open(builder.root / "setup.log", "ab") as log:
            builder.log = log
            builder.setup()
    elif args.command == "doctor":
        emit("health", **builder.doctor())
    elif args.command == "plan":
        builder.plan(builder.app(args.recipe), request)
    elif args.command == "enroll":
        app = Path(args.app or "")
        with tempfile.TemporaryDirectory(dir=builder.root) as tmp, open(builder.root / "enrollment.log", "ab") as log:
            builder.log = log
            ipa = Path(tmp) / "enrollment.ipa"
            installed_ipa(app, ipa)
            builder.command(["sudo", "-n", builder.tool("unfairplay"), "setup", "--app", app.resolve(), "--ipa", ipa, "--model-dir", builder.root / "models"], timeout=1800,
                            message="Enrollment failed; inspect the private enrollment log and UnfairPlay’s support requirements")
        emit("result", enrolled=True)
    else:
        if not args.job_id or not re.fullmatch(r"[0-9a-f]{24}", args.job_id):
            raise BuildError("Invalid build job identifier")
        job_dir = builder.tools / "jobs" / args.job_id
        job_dir.mkdir(mode=0o700, exist_ok=True)
        log_dir = builder.root / "jobs" / args.job_id
        log_dir.mkdir(mode=0o700, exist_ok=True)
        try:
            with open(log_dir / "build.log", "ab") as log:
                builder.log = log
                app = builder.app(args.recipe)
                if args.command == "compile":
                    optional = [p["id"] for p in app["packages"] if p.get("optional")]
                    products = builder.compile_tweaks(app, request.get("tweaks", optional), job_dir)
                    emit("result", compiled=len(products))
                else:
                    builder.build(app, request["plan"], request["sequence"], job_dir)
        finally:
            # Retain only the private diagnostic log. Account-personalized input and
            # decrypted intermediates must not accumulate after failures or cancellation.
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
        emit("error", message="The builder encountered an unexpected error; inspect the private Mac setup and build logs.", blocked=False)
        sys.exit(1)
