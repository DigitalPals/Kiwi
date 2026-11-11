#!/usr/bin/env python3
"""Community builds: the `community` command of android.py.

Kiwi's server hands this runner one job: an app (its package), the candidate bundles
from the Morphe community list it ranked by the list's own data, and whatever Kiwi
pinned for this app before. Everything else comes from the bundles themselves:

1. For at most three candidates, the latest stable GitHub release (addresses Kiwi
   builds from a checked `owner/repo`, never ones the list names), its one `.mpp`
   asset (size-capped, hashed), the patcher version its manifest declares and what
   `list-patches` says about it. The bundle is third-party code, so `list-patches`
   runs in the sandbox, which is required: no sandbox, no community build.
2. The best eligible candidate: newest target version Kiwi can download, then most
   patches on by default for this app, most recently released, fewest other apps,
   most stars.
3. The original at that version, from Google Play first. The publisher's certificate
   is pinned the first time Play delivers the app; APKPure is only tried once a
   certificate is pinned, and every file must match the pin.
4. The bundle's default patches applied in the sandbox, with only arm64-v8a native
   code kept; the patcher never sees Kiwi's key. Kiwi signs the output outside the
   sandbox and checks it against the pins.

When a candidate's build fails (patches fail, need the network, or expect another
original), the next eligible candidate is tried, three in all. Events tell the server
what was learned, so it can show the candidates and pin the result.
"""
import hashlib
import json
import os
from pathlib import Path
import re
import shutil
import stat
import time
import urllib.error
import urllib.parse
import urllib.request
import zipfile

import android
import apk
from runner import BuildError, canonical, digest, emit
import sandbox

# The patcher API inside the pinned Morphe (morphe-desktop 2.1.1 embeds morphe-patcher
# 2.3.0, app/morphe/patcher/version.properties); bundles declaring a newer one can't load.
EMBEDDED_PATCHER = "2.3.0"
PATCHER_SHA256 = "3a2efefd567b44dde15971a58d5aa649a32aba30781d3400dc08d688c720168f"
MAX_BUNDLE = 64 * 1024**2
MAX_LIST = 32 * 1024**2
MAX_GITHUB = 2 * 1024**2
MAX_INSPECT = 3
MAX_BUILDS = 3
ABI = "arm64-v8a"
ABI_KEY = "ARM64_V8A"
# Cached originals beyond this are not kept after a successful community build.
ORIGINALS_BUDGET = 4 * 1024**3
# Downloaded bundles and their patch lists unused for this long are deleted.
KEEP_DAYS = 30
MODES = ("auto", "manual", "pinned", "update")
PACKAGE = re.compile(r"[a-zA-Z][a-zA-Z0-9_]*(\.[a-zA-Z0-9_]+)+")
OWNER = re.compile(r"[A-Za-z0-9](?:[A-Za-z0-9]|-(?=[A-Za-z0-9])){0,38}")
REPO_NAME = re.compile(r"[A-Za-z0-9._-]{1,100}")
TAG = re.compile(r"[A-Za-z0-9][A-Za-z0-9._+/-]{0,127}")
SHA = re.compile(r"[0-9a-f]{64}")
ASSET = re.compile(r"[A-Za-z0-9][A-Za-z0-9._+-]{0,199}\.mpp", re.I)
# Release files download from here (not the API, so they don't count against its limit).
DOWNLOADS = "https://github.com"


class CommunityError(BuildError):
    """A build failure the server shows with what the owner can do (`kind`)."""

    def __init__(self, message, kind=None, blocked=False):
        super().__init__(message, blocked)
        self.kind = kind


class CertificateMismatch(android.SourceError):
    """An original signed by someone other than the pinned publisher."""


class CandidateFailed(Exception):
    """One bundle can't be used; the next eligible one may be. `kind` as CommunityError's."""

    def __init__(self, message, kind="other_bundle"):
        super().__init__(message)
        self.kind = kind


def valid_repo(repo):
    if not isinstance(repo, str) or repo.count("/") != 1:
        return False
    owner, name = repo.split("/")
    return bool(OWNER.fullmatch(owner) and REPO_NAME.fullmatch(name) and name not in (".", ".."))


def valid_tag(tag):
    return isinstance(tag, str) and bool(TAG.fullmatch(tag)) and ".." not in tag


def compare_versions(a, b):
    """-1, 0 or 1: version names compared part by part (numbers as numbers); "" (any
    version) is the lowest. The same order as Kiwi's server uses."""
    def parts(v):
        return [p for p in re.split(r"[.\-_ ]", v or "") if p]
    left, right = parts(a), parts(b)
    for i in range(max(len(left), len(right))):
        x = left[i] if i < len(left) else ""
        y = right[i] if i < len(right) else ""
        if x.isdigit() and y.isdigit():
            order = (int(x) > int(y)) - (int(x) < int(y))
        elif x.isdigit() and not y:
            order = 1
        elif y.isdigit() and not x:
            order = -1
        else:
            order = (x > y) - (x < y)
        if order:
            return order
    return 0


def version_key(version):
    """A sort key equivalent to compare_versions."""
    import functools
    return functools.cmp_to_key(compare_versions)(version)


# --- reading a bundle ---------------------------------------------------------------------

def read_manifest(path):
    """The main attributes of a bundle's JAR manifest (data only; the bundle never runs here)."""
    try:
        with zipfile.ZipFile(path) as archive:
            info = archive.getinfo("META-INF/MANIFEST.MF")
            if info.file_size > 64 * 1024:
                raise CandidateFailed("its bundle has an oversized manifest")
            text = archive.read(info).decode("utf-8", "replace")
    except KeyError:
        return {}
    except (zipfile.BadZipFile, OSError) as error:
        raise CandidateFailed("its bundle is not a readable patch bundle") from error
    attributes, last = {}, None
    for line in text.replace("\r\n", "\n").split("\n"):
        if not line:
            break  # the main section ends at the first blank line
        if line.startswith(" ") and last:
            attributes[last] += line[1:]
            continue
        name, _, value = line.partition(": ")
        if name and _:
            attributes[name.strip()] = value.strip()
            last = name.strip()
    return attributes


def patcher_too_new(declared):
    return bool(declared) and compare_versions(declared, EMBEDDED_PATCHER) > 0


def parse_list(text):
    """The patches `list-patches -p -v -x -o -i` prints, as dicts: name, description,
    enabled, and packages ({package: {"versions": [...], "codes": {version: {ABI: code}}}},
    or None for a universal patch)."""
    patches, current, section, package, sub = [], None, None, None, None
    for line in text.splitlines():
        if line.startswith("Name: "):
            current = {"name": line[6:].strip(), "description": "", "enabled": True, "packages": None}
            patches.append(current)
            section = package = sub = None
            continue
        if current is None:
            continue
        if line.startswith("Description: "):
            current["description"], section = line[13:].strip(), "description"
        elif line.startswith("Enabled: "):
            current["enabled"], section = line[9:].strip() == "true", None
        elif line.startswith("Options:"):
            section = "options"
        elif line.startswith("Compatible packages:"):
            current["packages"], section, package, sub = {}, "packages", None, None
        elif section == "packages" and line.startswith("\tPackage name: "):
            package = line.split(": ", 1)[1].strip()
            current["packages"].setdefault(package, {"versions": [], "codes": {}})
            sub = None
        elif section == "packages" and line.startswith("\tCompatible versions:"):
            sub = "versions"
        elif section == "packages" and line.startswith("\tVersion codes:"):
            sub = "codes"
        elif section == "packages" and line.startswith("\t\t") and package:
            value = line.strip()
            entry = current["packages"][package]
            if sub == "versions" and value and value not in entry["versions"]:
                entry["versions"].append(value)
            elif sub == "codes":
                name, _, codes = value.rpartition(": ")
                found = {}
                for pair in codes.split(","):
                    abi, _, code = pair.strip().partition("=")
                    if abi and code.isdigit():
                        found[abi] = int(code)
                if name and found:
                    entry["codes"][name] = found
        elif section == "description" and line.strip() and not line.startswith("\t"):
            current["description"] += " " + line.strip()
    return patches


def facts(patches, package):
    """What a bundle offers one app: patch counts, target versions (best first: supported by
    the most default patches, then newest) and the codes it declares per version."""
    specific = [p for p in patches if p["packages"] and package in p["packages"]]
    on = [p for p in specific if p["enabled"]]
    lists = [p["packages"][package]["versions"] for p in on]
    any_version = bool(on) and not any(lists)
    support = {}
    for versions in lists:
        for v in versions:
            support[v] = 0
    for v in support:
        support[v] = sum(1 for versions in lists if not versions or v in versions)
    ordered = sorted(support, key=lambda v: (support[v], version_key(v)), reverse=True)
    codes = {}
    for p in specific:
        for name, by_abi in p["packages"][package]["codes"].items():
            if ABI_KEY in by_abi:
                codes.setdefault(name, [])
                if by_abi[ABI_KEY] not in codes[name]:
                    codes[name].append(by_abi[ABI_KEY])
    return {"patches": len(specific), "default_patches": len(on), "versions": ordered, "any_version": any_version,
            "codes": codes, "descriptions": {p["name"]: p["description"] for p in patches}}


def rank(candidates):
    """Eligible candidates, best first (see the module's docstring)."""
    return sorted(candidates, key=lambda c: (version_key(c["target"]["version"]), c["default_patches"],
                                             c.get("published_at") or "", -c.get("targets", 0), c.get("stars", 0)),
                  reverse=True)


def codes_to_try(version, declared, play, pure):
    """Version codes for `version`, most trustworthy first: the bundle's own declaration
    for arm64, Play's current release when it has that name, then APKPure's list."""
    out = []
    for code in declared.get(version, []):
        if code not in out:
            out.append(code)
    if play and play.get("version") == version and play.get("code") and play["code"] not in out:
        out.append(play["code"])
    for entry in pure or []:
        if entry["version"] == version and entry["code"] not in out:
            out.append(entry["code"])
    return out


NETWORK_HINTS = ("UnknownHostException", "ConnectException", "SocketException", "BindException", "Permission denied (connect",
                 "didn't answer", "Network is unreachable")


def patch_failure(outcome):
    """Why the patcher's result says patches failed, in a few words: the patches' names, and
    whether one needed the network (which the sandbox doesn't have)."""
    failed = outcome.get("failedPatches") if isinstance(outcome, dict) else None
    if not isinstance(failed, list) or not failed:
        return None
    names, network = [], False
    for entry in failed[:50]:
        name = entry.get("name") if isinstance(entry, dict) else entry
        error = str(entry.get("error", "")) if isinstance(entry, dict) else ""
        network = network or any(hint in error for hint in NETWORK_HINTS)
        if isinstance(name, str) and len(names) < 3:
            names.append(name[:80])
    listed = ", ".join(names) + (" and more" if len(failed) > 3 else "")
    if network:
        return f"a patch needs the network while patching, which the sandbox doesn't allow ({listed})"
    return f"some of its patches failed ({listed})"


def read_result(path):
    """The patcher's result file, read as a plain file (not a link), or None."""
    try:
        info = os.lstat(path)
        if not stat.S_ISREG(info.st_mode) or info.st_size > android.MAX_RESULT:
            return None
        outcome = json.loads(Path(path).read_text())
        return outcome if isinstance(outcome, dict) else None
    except (OSError, ValueError):
        return None


def check_original(files, package, version, code, certificate):
    """The certificates of a complete original: every file is the app at that version code,
    one base APK has the version name, all files are signed alike and include arm64 code
    where there is native code. With a pinned certificate, it must be among them."""
    certificates, base, abis = None, 0, set()
    for path in files:
        try:
            found = apk.inspect(path)
        except (apk.ApkError, OSError) as error:
            raise android.SourceError(str(error)) from error
        if found["package"] != package:
            raise android.SourceError("it supplied a different app")
        if found["version_code"] != code:
            raise android.SourceError("it supplied a different version")
        if certificates is None:
            certificates = found["certificates"]
        elif sorted(found["certificates"]) != sorted(certificates):
            raise android.SourceError("its files are signed by different keys")
        abis.update(found["abis"])
        if "split" not in found:
            base += 1
            if found["version"] != version:
                raise android.SourceError("it supplied a different version")
    if base != 1:
        raise android.SourceError("it supplied an incomplete app")
    if abis and ABI not in abis:
        raise android.SourceError("it supplied the app for a different processor")
    if certificate and certificate not in certificates:
        raise CertificateMismatch("its files are not signed by the publisher Kiwi pinned for this app")
    return certificates


# --- the job ------------------------------------------------------------------------------

def check_request(request):
    if not isinstance(request, dict):
        raise BuildError("Invalid community build request")
    package = request.get("package")
    if not isinstance(package, str) or not PACKAGE.fullmatch(package) or len(package) > 255:
        raise BuildError("Invalid community app")
    if request.get("mode") not in MODES:
        raise BuildError("Invalid community build mode")
    if not isinstance(request.get("name"), str) or type(request.get("sequence")) is not int or request["sequence"] <= 0:
        raise BuildError("Invalid community build request")
    candidates = request.get("candidates")
    if not isinstance(candidates, list) or not candidates or len(candidates) > 10:
        raise BuildError("Invalid community candidates")
    for c in candidates:
        if not isinstance(c, dict) or not valid_repo(c.get("repo")):
            raise BuildError("Invalid community candidate")
    pins = request.get("pins")
    if pins is not None:
        if (not isinstance(pins, dict) or not valid_repo(pins.get("repo")) or type(pins.get("repo_id")) is not int
                or not valid_tag(pins.get("tag")) or not SHA.fullmatch(str(pins.get("asset_sha256")))
                or type(pins.get("version_code")) is not int or not isinstance(pins.get("version"), str)
                or not PACKAGE.fullmatch(str(pins.get("output_package")))
                or type(pins.get("output_version_code")) is not int):
            raise BuildError("Invalid community pins")
    if request["mode"] in ("pinned", "update") and pins is None:
        raise BuildError("Invalid community pins")
    if request["mode"] == "update" and not valid_tag(request.get("update_tag")):
        raise BuildError("Invalid community update")
    certificate = request.get("certificate")
    if certificate is not None and not SHA.fullmatch(str(certificate)):
        raise BuildError("Invalid pinned certificate")
    api = request.get("github_api")
    if not isinstance(api, str) or not (api.startswith("https://") or api.startswith("http://127.0.0.1:")):
        raise BuildError("Invalid GitHub address")
    return request


class Community:
    def __init__(self, base, request, job_dir):
        self.base = base  # android.Android
        self.builder = base.builder
        self.request = check_request(request)
        self.package = request["package"]
        self.name = request["name"]
        self.mode = request["mode"]
        self.pins = request.get("pins")
        self.certificate = request.get("certificate")
        self.api = request["github_api"].rstrip("/")
        self.skip = {k.lower(): v for k, v in (request.get("skip") or {}).items() if isinstance(v, str)}
        self.job_dir = job_dir
        self.store = base.tools / "community"
        for path in (self.store / "bundles", self.store / "lists"):
            path.mkdir(mode=0o700, parents=True, exist_ok=True)
        self.cache_path = base.private / "community-github.json"
        self._play_info = self._pure = None
        self._play = None

    def log(self, text):
        if self.builder.log is not None:
            self.builder.log.write(f"kiwi: community: {text}\n".encode())

    # --- GitHub -------------------------------------------------------------------------

    def github(self, path):
        """A GitHub API document (a small, parsed subset). Conditional requests answered
        with 304 don't count against GitHub's limit of 60 an hour, so answers are kept."""
        url = self.api + path
        try:
            cache = json.loads(self.cache_path.read_text())
            if not isinstance(cache, dict):
                cache = {}
        except (OSError, ValueError):
            cache = {}
        headers = {"User-Agent": "Kiwi", "Accept": "application/vnd.github+json", "X-GitHub-Api-Version": "2022-11-28"}
        kept = cache.get(url)
        if isinstance(kept, dict) and isinstance(kept.get("etag"), str):
            headers["If-None-Match"] = kept["etag"]
        status, body, reply = self.http_github(url, headers)
        if status == 304 and isinstance(kept, dict):
            return kept["body"]
        if status in (403, 429) and (reply.get("x-ratelimit-remaining") == "0" or status == 429):
            raise CommunityError("GitHub's limit for lookups without an account was reached. Try again in an hour.",
                                 "retry")
        if status == 404:
            raise android.SourceError("GitHub has no such repository or release")
        if status != 200:
            raise CommunityError(f"GitHub didn't answer (HTTP {status}). Try again later.", "retry")
        try:
            document = json.loads(body)
        except ValueError as error:
            raise CommunityError("GitHub sent an unreadable answer. Try again later.", "retry") from error
        small = self.subset(document)
        if reply.get("etag"):
            cache[url] = {"etag": reply["etag"], "body": small}
            if len(cache) > 300:
                for old in list(cache)[:-300]:
                    del cache[old]
            partial = self.cache_path.with_suffix(".partial")
            partial.write_text(json.dumps(cache))
            partial.replace(self.cache_path)
        return small

    @staticmethod
    def subset(document):
        if not isinstance(document, dict):
            raise CommunityError("GitHub sent an unexpected answer. Try again later.", "retry")
        if "tag_name" in document:
            assets = [{"id": a.get("id"), "name": a.get("name"), "size": a.get("size")}
                      for a in document.get("assets") or [] if isinstance(a, dict)][:100]
            return {k: document.get(k) for k in ("id", "tag_name", "draft", "prerelease", "published_at")} | {"assets": assets}
        return {"id": document.get("id"), "full_name": document.get("full_name"),
                "stargazers_count": document.get("stargazers_count"),
                "owner": {"login": (document.get("owner") or {}).get("login")}}

    def http_github(self, url, headers):
        """(status, body, lower-case headers) of one GitHub API request."""
        request = urllib.request.Request(url, headers=headers)
        try:
            with urllib.request.urlopen(request, timeout=60) as response:
                body = response.read(MAX_GITHUB + 1)
                if len(body) > MAX_GITHUB:
                    raise CommunityError("GitHub sent an oversized answer.", "retry")
                return response.status, body, {k.lower(): v for k, v in response.headers.items()}
        except urllib.error.HTTPError as error:
            return error.code, error.read(64 * 1024), {k.lower(): v for k, v in error.headers.items()}
        except (urllib.error.URLError, OSError, ValueError) as error:
            raise CommunityError("Kiwi couldn't reach GitHub. Check the connection and try again.", "retry") from error

    def release(self, candidate):
        """(repo id, release, asset) of a candidate: its latest stable release, or the pinned
        or approved tag. The repository's id is looked up only for a pinned bundle here;
        others get theirs once Kiwi builds them (repository_id), which saves GitHub requests."""
        repo = candidate["repo"]
        if self.pins and self.mode in ("pinned", "update"):
            info = self.github(f"/repositories/{self.pins['repo_id']}")
            if str(info.get("full_name", "")).lower() != self.pins["repo"].lower():
                raise CommunityError(f"The bundle's repository {self.pins['repo']} moved or changed its name. "
                                     "Choose its bundle again.", "other_bundle", True)
            repo_id = self.pins["repo_id"]
            tag = self.request["update_tag"] if self.mode == "update" else self.pins["tag"]
            release = self.github(f"/repositories/{repo_id}/releases/tags/{urllib.parse.quote(tag, safe='')}")
        else:
            repo_id = None
            release = self.github(f"/repos/{repo}/releases/latest")
        if release.get("draft") or release.get("prerelease") or not valid_tag(release.get("tag_name")):
            raise CandidateFailed("its latest release is not a stable release")
        bundles = sorted((a for a in release["assets"] if isinstance(a.get("name"), str)
                          and ASSET.fullmatch(a["name"]) and type(a.get("id")) is int),
                         key=lambda a: a["name"])
        if not bundles:
            raise CandidateFailed("its latest release has no patch bundle")
        asset = bundles[0]
        if type(asset.get("size")) is not int or not 0 < asset["size"] <= MAX_BUNDLE:
            raise CandidateFailed("its patch bundle is larger than Kiwi accepts")
        return repo_id, release, asset

    def repository_id(self, repo):
        """The numeric id of a repository, which the pins keep (it survives renames)."""
        info = self.github(f"/repos/{repo}")
        if type(info.get("id")) is not int or str(info.get("full_name", "")).lower() != repo.lower():
            raise CandidateFailed("GitHub has no such repository", "other_bundle")
        return info["id"]

    def bundle(self, repo, release, asset):
        """The bundle file of a release, from the address GitHub gives every release file (built
        from the checked repository, tag and file name), and its SHA-256."""
        partial = self.store / "bundles" / f"{asset['id']}.partial"
        url = (f"{DOWNLOADS}/{repo}/releases/download/{urllib.parse.quote(release['tag_name'], safe='')}/"
               f"{urllib.parse.quote(asset['name'], safe='')}")
        try:
            sha = android.download(url, partial, limit=MAX_BUNDLE)
        except android.SourceError as error:
            raise CommunityError("Kiwi couldn't download a patch bundle from GitHub. Try again later.", "retry") from error
        path = self.store / "bundles" / f"{sha}.mpp"
        partial.replace(path)
        return path, sha

    def prune(self, now=None):
        """Deletes bundles, patch lists and partial files no build used for KEEP_DAYS."""
        now = time.time() if now is None else now
        for folder in (self.store / "bundles", self.store / "lists"):
            for path in folder.iterdir():
                try:
                    info = os.lstat(path)
                    if stat.S_ISREG(info.st_mode) and now - info.st_mtime > KEEP_DAYS * 86400:
                        path.unlink()
                except OSError:
                    pass

    # --- inspecting a candidate ---------------------------------------------------------

    def list_patches(self, bundle, sha, java, patcher):
        """`list-patches` of a bundle, run in the sandbox (required); kept per bundle digest."""
        kept = self.store / "lists" / f"{sha}-{PATCHER_SHA256[:16]}.txt"
        if kept.is_file():
            os.utime(kept)
            return kept.read_text(errors="replace")
        box = self.job_dir / f"list-{sha[:12]}"
        shutil.rmtree(box, ignore_errors=True)
        for name in ("home", "tmp", "data"):
            (box / name).mkdir(mode=0o700, parents=True)
        try:
            self.base.confined(
                [java, *android.JVM, f"-Djava.io.tmpdir={box / 'tmp'}", f"-Duser.home={box / 'home'}", "-jar", patcher,
                 "list-patches", "-p", "-v", "-x", "-o", "-i", "--patches", bundle, "--out", box / "list.txt"],
                mode=sandbox.REQUIRED, read=[patcher, bundle], run=[java.parent.parent], write=[box], cwd=box,
                timeout=600, env=sandbox.environment(box / "home", box / "tmp", MORPHE_DATA_DIR=box / "data"),
                message="list-patches failed")
        except BuildError as error:
            if error.blocked:
                raise
            raise CandidateFailed("Morphe couldn't read its patch bundle") from error
        out = box / "list.txt"
        try:
            info = os.lstat(out)
            if not stat.S_ISREG(info.st_mode) or info.st_size > MAX_LIST:
                raise OSError("not a usable file")
            text = out.read_bytes().decode("utf-8", "replace")
        except OSError as error:
            raise CandidateFailed("Morphe listed no patches in its bundle") from error
        finally:
            shutil.rmtree(box, ignore_errors=True)
        partial = kept.with_suffix(".partial")
        partial.write_text(text)
        partial.replace(kept)
        return text

    def play_info(self):
        """Play's current version of the app: {"version", "code"}, or None."""
        if self._play_info is None:
            info = {}
            try:
                body = self.play().call("details", {"doc": self.package})
                doc = android.first(body, 1, 2, 4)
                details = android.first(doc, 13, 1)
                offer = android.first(doc, 8)
                price = android.first(offer, 1) if offer else None
                info = {"version": android.text(android.first(details, 4)), "code": android.first(details, 3),
                        "paid": isinstance(price, int) and price > 0}
                if not isinstance(info["code"], int):
                    info = {"missing": True}
            except android.SourceError as error:
                self.log(f"Play details: {error}")
                info = {"missing": True}
            self._play_info = info
        return None if self._play_info.get("missing") else self._play_info

    def pure_versions(self):
        if self._pure is None:
            self._pure = []
            try:
                status, body = android.http(android.PURE + urllib.parse.quote(self.package), limit=32 * 1024**2,
                                            headers={"x-cv": "3172501", "x-sv": "29", "x-abis": ABI, "x-gp": "1"})
                if status == 200:
                    for entry in android.nested(body, 1, 7, 2):
                        info = android.first(entry, 3, 2)
                        code = android.text(android.first(info, 5)) if info else ""
                        if code.isdigit():
                            self._pure.append({"version": android.text(android.first(info, 6)), "code": int(code)})
            except android.SourceError as error:
                self.log(f"APKPure versions: {error}")
        return self._pure

    def play(self):
        if self._play is None:
            self._play = android.Play(self.base.toolchain["play"], self.base.recipes["device"],
                                      self.base.private / "play-session.json")
        return self._play

    def target(self, found):
        """The version to build and the codes to try for it, or a reason there is none."""
        play = self.play_info()
        if play and play.get("paid"):
            raise CommunityError(f"{self.name} is a paid app; Kiwi can only download free apps.", "unavailable")
        if found["any_version"]:
            if not play:
                raise CandidateFailed(f"Google Play doesn't offer {self.name} here", "unavailable")
            return {"version": play["version"], "codes": [play["code"]]}
        for version in found["versions"]:
            codes = codes_to_try(version, found["codes"], play, None)
            if not codes:
                codes = codes_to_try(version, found["codes"], play, self.pure_versions())
            if codes:
                return {"version": version, "codes": codes}
        raise CandidateFailed(f"none of the {self.name} versions it supports can be downloaded", "unavailable")

    def inspect(self, candidate, java, patcher):
        """A candidate's facts from its pinned or latest release; raises CandidateFailed
        when it is not eligible (after telling the server)."""
        repo = candidate["repo"]
        emit("stage", stage=f"Checking {repo}")
        repo_id, release, asset = self.release(candidate)
        tag = release["tag_name"]
        if self.mode == "auto" and self.skip.get(repo.lower()) == tag:
            return None
        event = {"repo": repo, "repo_id": repo_id, "tag": tag, "published_at": release.get("published_at")}
        try:
            path, sha = self.bundle(self.pins["repo"] if self.pins and self.mode in ("pinned", "update") else repo,
                                    release, asset)
            if self.pins and self.mode == "pinned" and sha != self.pins["asset_sha256"]:
                raise CommunityError("The bundle's files changed without a new release, so Kiwi stopped. Choose "
                                     "another bundle, or wait for a new release.", "other_bundle", True)
            declared = read_manifest(path).get("Patcher-Version", "")
            event["patcher_version"] = declared or None
            if patcher_too_new(declared):
                raise CandidateFailed(f"it needs a newer Morphe patcher ({declared})")
            found = facts(parse_list(self.list_patches(path, sha, java, patcher)), self.package)
            event.update(versions=found["versions"][:10], patches=found["patches"], default_patches=found["default_patches"])
            if not found["default_patches"]:
                raise CandidateFailed(f"none of its patches for {self.name} are on by default")
            if self.mode == "pinned":
                chosen = {"version": self.pins["version"], "codes": [self.pins["version_code"]]}
            else:
                chosen = self.target(found)
        except CandidateFailed as error:
            emit("community", kind="candidate", eligible=False, reason=str(error), **event)
            raise
        emit("community", kind="candidate", eligible=True, reason=None, **event)
        return dict(candidate, repo_id=repo_id, tag=tag, published_at=release.get("published_at"), asset=asset["name"],
                    asset_sha256=sha, path=path, target=chosen, patches=found["patches"],
                    default_patches=found["default_patches"], descriptions=found["descriptions"])

    # --- the original -------------------------------------------------------------------

    def original(self, target):
        """(files, source, code) of the original at the target version."""
        version = target["version"]
        failures = []
        for code in target["codes"]:
            cache = self.base.tools / "originals" / self.package / str(code)
            marker = cache.with_name(cache.name + ".complete")
            if marker.is_file() and self.certificate:
                files = sorted(cache.glob("*.apk"))
                try:
                    check_original(files, self.package, version, code, self.certificate)
                    return files, marker.read_text(), code
                except android.SourceError:
                    pass
            sources = [("Google Play", lambda t, c=code: self.play().fetch(self.package, c, t))]
            if self.certificate:
                # Only once Play has shown Kiwi the publisher's certificate.
                sources.append(("APKPure", lambda t, c=code: android.apkpure(self.package, version, c, ABI, t)))
            for index, (name, fetch) in enumerate(sources):
                work = self.job_dir / f"original-{code}-{index}"
                shutil.rmtree(work, ignore_errors=True)
                work.mkdir(mode=0o700, parents=True)
                try:
                    files = [f for f in fetch(work) if f.suffix == ".apk"]
                    certificates = check_original(files, self.package, version, code, self.certificate)
                    if not self.certificate:
                        if name != "Google Play" or len(certificates) != 1:
                            raise android.SourceError("its certificate can't be pinned")
                        self.certificate = certificates[0]
                        emit("community", kind="certificate", sha256=self.certificate)
                except CertificateMismatch as error:
                    if name == "Google Play":
                        raise CommunityError(f"Google Play delivered {self.name} signed by another publisher than the "
                                             "one Kiwi pinned for it, so Kiwi stopped.", "unavailable", True) from error
                    failures.append(f"{name} ({code}): {error}")
                    shutil.rmtree(work, ignore_errors=True)
                    continue
                except android.SourceError as error:
                    failures.append(f"{name} ({code}): {error}")
                    self.log(f"{name} {code}: {error}")
                    shutil.rmtree(work, ignore_errors=True)
                    continue
                for stale in cache.parent.glob("*") if cache.parent.is_dir() else []:
                    shutil.rmtree(stale) if stale.is_dir() else stale.unlink()
                cache.mkdir(mode=0o700, parents=True)
                kept = []
                for number, path in enumerate(files):
                    kept.append(cache / f"{number:03d}.apk")
                    shutil.copyfile(path, kept[-1])
                marker.write_text(name)
                shutil.rmtree(work, ignore_errors=True)
                return kept, name, code
        self.log("no original: " + "; ".join(failures))
        raise CandidateFailed(f"Kiwi couldn't download {self.name} {version} from Google Play", "unavailable")

    def trim_originals(self, files):
        """Drops the original just used when the cached originals exceed their budget."""
        root = self.base.tools / "originals"
        total = sum(p.stat().st_size for p in root.rglob("*.apk") if p.is_file())
        if total > ORIGINALS_BUDGET and files:
            cache = files[0].parent
            shutil.rmtree(cache, ignore_errors=True)
            cache.with_name(cache.name + ".complete").unlink(missing_ok=True)

    # --- building -----------------------------------------------------------------------

    def build_candidate(self, chosen, java, patcher, signer_jar, keystore):
        """Patches, signs and checks one candidate; returns the artifact event's fields."""
        store, password, signer = keystore
        emit("stage", stage=f"Downloading original {self.name}")
        files, source, code = self.original(chosen["target"])
        version = chosen["target"]["version"]
        self.attempts = getattr(self, "attempts", 0) + 1
        n = self.attempts
        work = self.job_dir / f"build-{n}"
        work.mkdir(mode=0o700)
        if len(files) == 1:
            original = files[0]
        else:
            original = work / "original.apks"
            with zipfile.ZipFile(original, "w", zipfile.ZIP_STORED) as bundle:
                for path in files:
                    bundle.write(path, path.name)
        emit("stage", stage=f"Patching {self.name}")
        box = self.job_dir / f"patch-{n}"
        for name in ("home", "tmp", "data"):
            (box / name).mkdir(mode=0o700, parents=True)
        try:
            self.base.confined(
                [java, *android.JVM, f"-Djava.io.tmpdir={box / 'tmp'}", f"-Duser.home={box / 'home'}", "-jar", patcher,
                 "patch", "--patches", chosen["path"], "--out", box / "unsigned.apk", "--temporary-files-path",
                 box / "work", "--result-file", box / "result.json", "--unsigned", "--striplibs", ABI, original],
                mode=sandbox.REQUIRED, read=[patcher, chosen["path"], original], run=[java.parent.parent], write=[box],
                cwd=box, timeout=3600, env=sandbox.environment(box / "home", box / "tmp", MORPHE_DATA_DIR=box / "data"),
                message="patch failed")
        except BuildError as error:
            if error.blocked:
                raise
            reason = patch_failure(read_result(box / "result.json"))
            raise CandidateFailed(reason or "the patcher stopped with an error") from error
        unsigned, result = work / "unsigned.apk", work / "result.json"
        for made, kept in ((box / "unsigned.apk", unsigned), (box / "result.json", result)):
            try:
                if not stat.S_ISREG(os.lstat(made).st_mode):
                    raise OSError("not a file")
                os.replace(made, kept)
            except OSError as error:
                raise CandidateFailed("the patcher produced no app") from error
        shutil.rmtree(box, ignore_errors=True)
        try:
            if result.stat().st_size > android.MAX_RESULT:
                raise ValueError("oversized")
            outcome = json.loads(result.read_text())
            if not isinstance(outcome, dict):
                raise ValueError("not an object")
        except (OSError, ValueError) as error:
            raise CandidateFailed("the patcher reported no result") from error
        applied = outcome.get("appliedPatches")
        if not isinstance(applied, list) or not all(isinstance(p, str) for p in applied):
            raise CandidateFailed("the patcher reported an unreadable result")
        if outcome.get("failedPatches"):
            raise CandidateFailed(patch_failure(outcome))
        if outcome.get("success") is not True or not applied:
            raise CandidateFailed("no patches applied")
        try:
            before = apk.unsigned(unsigned)
        except (apk.ApkError, OSError) as error:
            raise CandidateFailed(f"the patched app is not a valid unsigned package: {error}") from error
        emit("stage", stage="Signing")
        candidate = work / "candidate.apk"
        # Kiwi's key reaches only apksigner, a pinned tool, through its environment.
        self.base.confined(
            [java, *android.JVM, f"-Djava.io.tmpdir={work}", "-jar", signer_jar, "sign", "--ks", store, "--ks-type",
             "PKCS12", "--ks-key-alias", android.ALIAS, "--ks-pass", "env:KIWI_KEYSTORE", "--key-pass",
             "env:KIWI_KEYSTORE", "--v1-signing-enabled", "false", "--v2-signing-enabled", "true",
             "--v3-signing-enabled", "true", "--v4-signing-enabled", "false", "--out", candidate, unsigned],
            mode=sandbox.REQUIRED, read=[signer_jar, store, unsigned], run=[java.parent.parent], write=[work], cwd=work,
            timeout=600, env=sandbox.environment(work, work, KIWI_KEYSTORE=password),
            message="apksigner could not sign the patched app; details are in the private log.")
        emit("stage", stage="Validating the patched app")
        try:
            patched = apk.inspect(candidate)
            unaligned = apk.misaligned(candidate)
        except (apk.ApkError, OSError) as error:
            raise BuildError("The patched app is not a valid signed package") from error
        self.log(f"patched app: {json.dumps(patched)}")
        if patched["certificates"] != [signer] or not {2, 3} <= set(patched["schemes"]):
            raise BuildError("The patched app is not signed with Kiwi's key")
        if unaligned:
            raise BuildError("The patched app is not aligned (" + ", ".join(unaligned[:3]) + ")")
        if any(patched[k] != before[k] for k in ("package", "version", "version_code", "abis")):
            raise BuildError("Signing changed the patched app unexpectedly")
        if not PACKAGE.fullmatch(patched["package"]) or patched["version"] != version:
            raise CandidateFailed("its patches changed the app's identity")
        if patched["abis"] and ABI not in patched["abis"]:
            raise CandidateFailed("the patched app lost its native code")
        if self.pins and self.mode in ("pinned", "update") and patched["package"] != self.pins["output_package"]:
            raise CommunityError("The patched app installs under another name than the pinned build. Kiwi stopped; "
                                 "choose the bundle again.", "other_bundle", True)
        if (self.pins and self.mode == "pinned" and chosen["tag"] == self.pins["tag"]
                and patched["version_code"] != self.pins["output_version_code"]):
            raise CommunityError("The patched app has another version code than the pinned build. Kiwi stopped; "
                                 "choose the bundle again.", "other_bundle", True)
        sha = digest(candidate)
        artifact = self.builder.root / "artifacts" / f"{sha}.apk"
        os.replace(candidate, artifact)
        patches = [{"name": name[:120], "description": chosen["descriptions"].get(name, "")[:1000]}
                   for name in sorted(applied)]
        pins = {"repo": chosen["repo"], "repo_id": chosen["repo_id"], "author": chosen.get("author") or chosen["repo"].split("/")[0],
                "tag": chosen["tag"], "published_at": chosen.get("published_at"), "asset": chosen["asset"],
                "asset_sha256": chosen["asset_sha256"], "bundle_name": chosen["repo"].split("/")[1],
                "version": version, "version_code": code, "output_package": patched["package"],
                "output_version_code": patched["version_code"], "patches": patches}
        provenance = {"community": pins, "sequence": self.request["sequence"], "source": source,
                      "original_sha256": [digest(p) for p in files], "sha256": sha, "built_at": time.time(),
                      "signer": signer, "certificate": self.certificate, "runtime_tested": False}
        (self.builder.root / "artifacts" / f"{sha}.json").write_bytes(canonical(provenance))
        self.trim_originals(files)
        return {"sha256": sha, "version": version, "build": str(patched["version_code"]),
                "revision": self.request["sequence"], "min_os": None, "size": artifact.stat().st_size,
                "package": patched["package"], "signer": signer, "community": pins,
                "notes": f"{self.name} {version} · Android build {code}\n"
                         f"Original from {source}, verified against the publisher's certificate Kiwi pinned.\n"
                         f"{len(applied)} default patches from {chosen['repo']} {chosen['tag']} (community, not "
                         f"reviewed); signed with this Kiwi's own key.\nNot yet tested on a physical device."}

    def run(self):
        self.prune()
        candidates = self.request["candidates"]
        if self.mode in ("auto", "manual"):
            emit("stage", stage="Choosing patches")
        java = self.base.java()
        patcher = self.base.tool("patcher")
        if digest(patcher) != PATCHER_SHA256:
            raise BuildError("The pinned Morphe patcher changed; community builds need the one Kiwi checked.", True)
        signer_jar = self.base.apksigner()
        keystore = self.base.keystore(java)
        limit = 1 if self.mode != "auto" else MAX_INSPECT
        eligible, reasons = [], []
        for candidate in candidates:
            if len(reasons) + len(eligible) >= limit:
                break
            try:
                found = self.inspect(candidate, java, patcher)
            except CandidateFailed as error:
                reasons.append((candidate["repo"], str(error), error.kind))
                continue
            except android.SourceError as error:
                if self.mode != "auto":
                    raise CommunityError(f"Kiwi couldn't use {candidate['repo']}: {error}.", "other_bundle") from error
                reasons.append((candidate["repo"], str(error), "other_bundle"))
                continue
            if found is not None:
                eligible.append(found)
        failures = []
        for chosen in rank(eligible)[:MAX_BUILDS]:
            emit("community", kind="chosen", repo=chosen["repo"], tag=chosen["tag"])
            try:
                if chosen["repo_id"] is None:
                    chosen["repo_id"] = self.repository_id(chosen["repo"])
                return self.build_candidate(chosen, java, patcher, signer_jar, keystore)
            except CandidateFailed as error:
                self.log(f"{chosen['repo']} {chosen['tag']}: {error}")
                # A download problem is not the bundle's fault; it is not marked failed.
                if error.kind != "unavailable":
                    emit("community", kind="failed", repo=chosen["repo"], tag=chosen["tag"], reason=str(error))
                failures.append((chosen["repo"], str(error), error.kind))
        raise self.failure(failures or reasons)

    def failure(self, failures):
        """The error for a job where no candidate built: `failures` are the builds that
        failed, or, when none was tried, why each candidate wasn't eligible."""
        if not failures:
            return CommunityError(f"No community bundle offers patches for {self.name} that Kiwi can use.", "unavailable")
        kinds = {kind for _, _, kind in failures}
        detail = "; ".join(f"{repo}: {reason}" for repo, reason, _ in failures[:3])
        if self.mode in ("manual", "pinned", "update"):
            repo, reason, kind = failures[0]
            return CommunityError(f"{repo} didn't build: {reason}.", kind)
        if kinds == {"unavailable"}:
            return CommunityError(f"No bundle works with a {self.name} version Kiwi can download ({detail}).", "unavailable")
        return CommunityError(f"No bundle built {self.name} ({detail}). Choose another bundle, or try again after "
                              "their next releases.", "other_bundle")


def run(builder, request, job_dir):
    """The `community` command: builds the app and emits its artifact."""
    community = Community(android.Android(builder), request, job_dir)
    emit("artifact", **community.run())
