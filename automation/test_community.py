from contextlib import redirect_stdout
import io
import json
from pathlib import Path
import shutil
import tempfile
import unittest
import zipfile

import android
import apk
import community
from runner import BuildError, Builder
import sandbox
from test_android import KIWI_KEY, PUBLISHER, STRANGER, package, unsigned_package

LIST = """Name: Venus settings
Description: Adds Venus to Discord's settings, where you turn each feature on or off.
Enabled: true
Compatible packages:
\tPackage name: com.discord
\tCompatible versions:
\t\t351.11 - Stable
\tVersion codes:
\t\t351.11 - Stable: ARM64_V8A=351011, ARMEABI_V7A=351010

Name: Custom branding
Description: Installs Venus as its own app.
Enabled: false
Options:
\tTitle: appName
\tDescription: The name.
\tRequired: true
\tKey: appName
\tDefault: Venus
\tType: String
Compatible packages:
\tPackage name: com.discord
\tCompatible versions:
\t\t351.11 - Stable

Name: Old feature
Description: Works on older versions too
and keeps its description over two lines.
Enabled: true
Compatible packages:
\tPackage name: com.discord
\tCompatible versions:
\t\t351.11 - Stable
\t\t350.0
\tPackage name: com.other.app

Name: Change app name
Description: Any app.
Enabled: false
"""


def bundle(path, patcher="2.3.0"):
    with zipfile.ZipFile(path, "w") as archive:
        archive.writestr("id", path.name)
        archive.writestr("META-INF/MANIFEST.MF", f"Manifest-Version: 1.0\r\nPatcher-Version: {patcher}\r\n"
                                                 "Name: A very long bundle name that wraps\r\n  onto a second line\r\n\r\n")
        archive.writestr("app/Patches.class", b"code")
    return path


class ParsingTests(unittest.TestCase):
    def test_list_patches_output_is_read_per_package(self):
        patches = community.parse_list(LIST)
        self.assertEqual([p["name"] for p in patches], ["Venus settings", "Custom branding", "Old feature", "Change app name"])
        self.assertEqual(patches[0]["packages"]["com.discord"]["codes"], {"351.11 - Stable": {"ARM64_V8A": 351011, "ARMEABI_V7A": 351010}})
        self.assertFalse(patches[1]["enabled"])
        self.assertEqual(patches[2]["description"], "Works on older versions too and keeps its description over two lines.")
        self.assertEqual(patches[2]["packages"]["com.other.app"], {"versions": [], "codes": {}})
        self.assertIsNone(patches[3]["packages"], "a universal patch")
        found = community.facts(patches, "com.discord")
        self.assertEqual((found["patches"], found["default_patches"]), (3, 2))
        self.assertEqual(found["versions"], ["351.11 - Stable", "350.0"], "the version every default patch supports first")
        self.assertEqual(found["codes"], {"351.11 - Stable": [351011]}, "only arm64 codes")
        self.assertFalse(found["any_version"])
        other = community.facts(patches, "com.other.app")
        self.assertTrue(other["any_version"])
        self.assertEqual(community.facts(patches, "com.nothing")["default_patches"], 0)

    def test_a_bundle_manifest_names_its_patcher(self):
        with tempfile.TemporaryDirectory() as folder:
            path = bundle(Path(folder) / "b.mpp", patcher="2.4.0")
            found = community.read_manifest(path)
            self.assertEqual(found["Patcher-Version"], "2.4.0")
            self.assertEqual(found["Name"], "A very long bundle name that wraps onto a second line")
            self.assertTrue(community.patcher_too_new("2.4.0"))
            self.assertFalse(community.patcher_too_new("2.3.0"))
            self.assertFalse(community.patcher_too_new("1.3.2"))
            self.assertFalse(community.patcher_too_new(""))
            (Path(folder) / "bad.mpp").write_bytes(b"not a zip")
            with self.assertRaises(community.CandidateFailed):
                community.read_manifest(Path(folder) / "bad.mpp")

    def test_failed_patches_are_named_and_network_needs_recognised(self):
        error = 'PatchException: Couldn\'t download Stockfish 19: github.com didn\'t answer (UnknownHostException: github.com)'
        self.assertEqual(community.patch_failure({"failedPatches": [{"name": "Stockfish analysis", "error": error}]}),
                         "a patch needs the network while patching, which the sandbox doesn't allow (Stockfish analysis)")
        self.assertEqual(community.patch_failure({"failedPatches": [{"name": n, "error": "x"} for n in "abcd"]}),
                         "some of its patches failed (a, b, c and more)")
        self.assertIsNone(community.patch_failure({"failedPatches": []}))
        self.assertIsNone(community.patch_failure(None))

    def test_versions_compare_like_the_server(self):
        self.assertEqual(community.compare_versions("351.11 - Stable", "351.9"), 1)
        self.assertEqual(community.compare_versions("12.40.1-prod.01", "12.41.0-prod.01"), -1)
        self.assertEqual(community.compare_versions("1.2", "1.2.1"), -1)
        self.assertEqual(community.compare_versions("", "0.1"), -1)

    def test_candidates_rank_by_version_then_default_patches_then_freshness(self):
        def c(repo, version, patches, published, targets=1, stars=0):
            return {"repo": repo, "target": {"version": version}, "default_patches": patches, "published_at": published,
                    "targets": targets, "stars": stars}
        ranked = community.rank([
            c("old/version", "31.7.0", 99, "2026-11-01T00:00:00Z"),
            c("thin/bundle", "31.8.0", 4, "2026-11-09T00:00:00Z"),
            c("lurk/bundle", "31.8.0", 42, "2026-11-01T00:00:00Z"),
            c("lurk/fork", "31.8.0", 42, "2026-11-01T00:00:00Z", targets=5),
        ])
        self.assertEqual([r["repo"] for r in ranked], ["lurk/bundle", "lurk/fork", "thin/bundle", "old/version"])

    def test_version_codes_come_from_the_bundle_then_play_then_apkpure(self):
        declared = {"1.0": [10]}
        play = {"version": "1.0", "code": 11}
        pure = [{"version": "1.0", "code": 12}, {"version": "1.0", "code": 10}, {"version": "0.9", "code": 9}]
        self.assertEqual(community.codes_to_try("1.0", declared, play, pure), [10, 11, 12])
        self.assertEqual(community.codes_to_try("0.9", declared, play, pure), [9])
        self.assertEqual(community.codes_to_try("0.8", declared, play, pure), [])

    def test_requests_are_checked(self):
        good = {"package": "com.discord", "name": "Discord", "sequence": 1, "mode": "auto",
                "candidates": [{"repo": "a/b"}], "pins": None, "certificate": None, "github_api": "https://api.github.com"}
        self.assertEqual(community.check_request(good), good)
        for change in ({"package": "../etc"}, {"mode": "anything"}, {"candidates": []}, {"candidates": [{"repo": "../x"}]},
                       {"github_api": "http://evil.example"}, {"certificate": "nope"}, {"mode": "pinned"},
                       {"sequence": 0}, {"mode": "update", "pins": None}):
            with self.assertRaises(BuildError, msg=change):
                community.check_request(dict(good, **change))
        for repo in ("a-/b", "-a/b", "a--b/c", "a/..", "a/b/c"):
            self.assertFalse(community.valid_repo(repo), repo)
        self.assertTrue(community.valid_repo("VenusIsJaded/Venus-Patches"))


class BuildTests(unittest.TestCase):
    """A community build with GitHub, Google Play and the tools stood in for."""

    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)
        self.root = Path(self.dir.name)
        self.builder = Builder(self.root / "data")
        self.builder.log = io.BytesIO()
        self.job = self.root / "job"
        self.job.mkdir()
        self.files = self.root / "files"
        self.files.mkdir()
        self.addCleanup(setattr, sandbox, "_probed", None)
        self.addCleanup(setattr, sandbox, "PLATFORM", sandbox.PLATFORM)
        sandbox.PLATFORM, sandbox._probed = "linux", {"landlock": 6, "seccomp": True, "reasons": []}
        self.addCleanup(setattr, sandbox, "kill_strays", sandbox.kill_strays)
        sandbox.kill_strays = lambda: 0
        originals = (android.download, android.http)
        self.addCleanup(lambda: (setattr(android, "download", originals[0]), setattr(android, "http", originals[1])))
        android.http = self.offline
        self.addCleanup(setattr, android, "apkpure", android.apkpure)

        def no_apkpure(*_args):
            raise android.SourceError("it does not offer this version")
        android.apkpure = no_apkpure
        # GitHub: two bundles, each with one release.
        self.bundles = {}
        self.releases = {}
        for number, repo in ((1, "good/bundle"), (2, "bad/bundle"), (3, "other/bundle")):
            path = bundle(self.files / f"{number}.mpp")
            self.bundles[number] = path
            self.releases[repo] = {"id": number, "full_name": repo, "tag": f"v1.{number}.0", "asset": 100 + number}
        self.lists = {1: LIST, 2: LIST, 3: LIST.replace("Enabled: true", "Enabled: false")}
        self.failing = set()
        self.github_calls = []
        self.commands = []
        self.served = {}  # code -> (key, version)
        self.served[351011] = (PUBLISHER, "351.11 - Stable")
        self.play_calls = []

    def offline(self, url, *_args, **_kwargs):
        raise AssertionError(f"unexpected request to {url}")

    def community(self, **request):
        request = {"package": "com.discord", "name": "Discord", "sequence": 3, "mode": "auto",
                   "candidates": [{"repo": "bad/bundle", "stars": 900}, {"repo": "good/bundle"}, {"repo": "other/bundle"}],
                   "pins": None, "certificate": None, "github_api": "https://api.github.com", **request}
        base = android.Android(self.builder)
        base.java = lambda: self.root / "jre" / "bin" / "java"
        base.tool = lambda name: self.patcher
        base.apksigner = lambda: self.root / "tools" / "apksigner.jar"
        self.store = self.root / "data" / "automation" / "android" / "keystore.p12"
        base.keystore = lambda java: (self.store, "secret-password", KIWI_KEY.fingerprint)
        base.builder.command = self.command
        self.patcher = self.root / "tools" / "morphe.jar"
        self.patcher.parent.mkdir(exist_ok=True)
        self.patcher.write_bytes(b"patcher")
        made = community.Community(base, request, self.job)
        made.http_github = self.github
        made.play_info = lambda: {"version": "353.11 - Stable", "code": 353011, "paid": False}
        made.pure_versions = lambda: []
        made.play = lambda: self
        community.PATCHER_SHA256 = android.digest(self.patcher)
        android.download = self.download
        return made

    def github(self, url, headers):
        self.github_calls.append(url)
        path = url.removeprefix("https://api.github.com")
        for repo, r in self.releases.items():
            if path in (f"/repos/{repo}", f"/repositories/{r['id']}"):
                return 200, json.dumps({"id": r["id"], "full_name": repo, "owner": {"login": repo.split("/")[0]}}).encode(), {}
            if path in (f"/repos/{repo}/releases/latest", f"/repositories/{r['id']}/releases/tags/{r['tag']}"):
                return 200, json.dumps({"id": 5, "tag_name": r["tag"], "draft": False, "prerelease": False,
                                        "published_at": f"2026-11-0{r['id']}T00:00:00Z",
                                        "assets": [{"id": r["asset"], "name": "patches.mpp", "size": 4000},
                                                   {"id": 9, "name": "notes.txt", "size": 1}]}).encode(), {}
        return 404, b"{}", {}

    def download(self, url, target, **_):
        self.downloads = getattr(self, "downloads", []) + [url]
        repo = url.removeprefix("https://github.com/").split("/releases/")[0]
        number = self.releases[repo]["id"]
        shutil.copyfile(self.bundles[number], target)
        return android.digest(target)

    # Google Play stand-in.
    def fetch(self, package_name, code, target):
        self.play_calls.append(code)
        if code not in self.served:
            raise android.SourceError("it does not offer this version")
        key, version = self.served[code]
        return [package(target / "base.apk", key, name=package_name, code=code, version=version),
                package(target / "split.apk", key, name=package_name, code=code, version=version, split="config.arm64_v8a")]

    def command(self, args, env=None, **options):
        args = [str(a) for a in args]
        spec, inner = json.loads(args[5]), args[7:]
        self.commands.append((inner, spec, env, options))
        if "list-patches" in inner:
            number = next(n for n, p in self.bundles.items() if str(p) in inner or any(
                a.endswith(".mpp") and android.digest(Path(a)) == android.digest(p) for a in inner))
            Path(inner[inner.index("--out") + 1]).write_text(self.lists[number])
        elif "patch" in inner:
            box = Path(inner[inner.index("--out") + 1]).parent
            mpp = inner[inner.index("--patches") + 1]
            if android.digest(Path(mpp)) in {android.digest(self.bundles[n]) for n in self.failing}:
                (box / "result.json").write_text(json.dumps({"success": False, "appliedPatches": ["A"],
                                                             "failedPatches": [{"name": "Stockfish analysis"}]}))
                unsigned_package(box / "unsigned.apk", name="com.discord", code=351011, version="351.11 - Stable")
                return ""
            unsigned_package(box / "unsigned.apk", name="com.discord", code=351011, version="351.11 - Stable")
            (box / "result.json").write_text(json.dumps({"success": True, "appliedPatches": ["Venus settings", "Old feature"],
                                                         "failedPatches": []}))
        elif "sign" in inner:
            facts = apk.unsigned(inner[-1])
            package(Path(inner[inner.index("--out") + 1]), KIWI_KEY, name=facts["package"], code=facts["version_code"],
                    version=facts["version"], abi=None, scheme=(2, 3), compression=zipfile.ZIP_DEFLATED)
        return ""

    def run_build(self, made):
        with redirect_stdout(io.StringIO()) as out:
            try:
                artifact = made.run()
            finally:
                self.events = [json.loads(line) for line in out.getvalue().splitlines()]
        return artifact

    def test_the_best_bundle_builds_and_its_pins_are_reported(self):
        self.failing = set()
        made = self.community()
        artifact = self.run_build(made)
        candidates = [e for e in self.events if e.get("kind") == "candidate"]
        self.assertEqual([(c["repo"], c["eligible"]) for c in candidates],
                         [("bad/bundle", True), ("good/bundle", True), ("other/bundle", False)])
        self.assertEqual(candidates[2]["reason"], "none of its patches for Discord are on by default")
        self.assertEqual(candidates[0]["versions"], ["351.11 - Stable", "350.0"])
        # Both eligible bundles target the same version with the same patches: the more
        # recent release wins.
        chosen = [e for e in self.events if e.get("kind") == "chosen"]
        self.assertEqual(chosen[0]["repo"], "bad/bundle")
        pins = artifact["community"]
        self.assertEqual((pins["repo"], pins["repo_id"], pins["tag"]), ("bad/bundle", 2, "v1.2.0"))
        self.assertEqual(pins["asset_sha256"], android.digest(self.bundles[2]))
        self.assertEqual((pins["version"], pins["version_code"]), ("351.11 - Stable", 351011), "the code the bundle declares")
        self.assertEqual((pins["output_package"], pins["output_version_code"]), ("com.discord", 351011))
        self.assertEqual([p["name"] for p in pins["patches"]], ["Old feature", "Venus settings"])
        self.assertEqual(artifact["package"], "com.discord")
        certificate = [e for e in self.events if e.get("kind") == "certificate"]
        self.assertEqual(certificate[0]["sha256"], PUBLISHER.fingerprint, "pinned from Google Play")
        self.assertEqual(self.play_calls, [351011])
        published = self.builder.root / "artifacts" / f"{artifact['sha256']}.apk"
        self.assertEqual(apk.inspect(published)["certificates"], [KIWI_KEY.fingerprint])
        # GitHub addresses are built from the checked owner/repo and numeric ids only.
        self.assertIn("https://api.github.com/repos/bad/bundle/releases/latest", self.github_calls)
        self.assertIn("https://github.com/bad/bundle/releases/download/v1.2.0/patches.mpp", self.downloads)
        # Only the bundle that was built needed its repository's id.
        self.assertIn("https://api.github.com/repos/bad/bundle", self.github_calls)
        self.assertNotIn("https://api.github.com/repos/good/bundle", self.github_calls)

    def test_bundle_code_runs_only_in_the_required_sandbox_without_the_key(self):
        made = self.community()
        self.run_build(made)
        listed = [c for c in self.commands if "list-patches" in c[0]]
        patched = [c for c in self.commands if "patch" in c[0]]
        self.assertTrue(listed and patched)
        for inner, spec, env, _ in listed + patched:
            self.assertTrue(spec["landlock"] and spec["seccomp"])
            self.assertEqual(len(spec["write"]), 1)
            self.assertNotIn(str(self.store), json.dumps(spec) + " ".join(inner))
            self.assertNotIn("secret-password", json.dumps(env) + " ".join(inner))
            self.assertNotIn("KIWI_KEYSTORE", env)
        self.assertIn("--striplibs", patched[0][0])
        self.assertEqual(patched[0][0][patched[0][0].index("--striplibs") + 1], "arm64-v8a")
        self.assertNotIn("--enable", patched[0][0], "the bundle's default patches")
        # Without a sandbox nothing runs.
        sandbox._probed = {"landlock": None, "seccomp": False, "reasons": ["Landlock is blocked here"]}
        self.commands.clear()
        self.job = self.root / "job2"
        self.job.mkdir()
        shutil.rmtree(self.root / "data" / "automation" / "tools" / "community" / "lists", ignore_errors=True)
        with self.assertRaises(BuildError) as raised:
            self.run_build(self.community())
        self.assertTrue(raised.exception.blocked)
        self.assertIn("only inside a sandbox", str(raised.exception))
        self.assertFalse([c for c in self.commands if "patch" in c[0] or "list-patches" in c[0]])

    def test_a_failed_bundle_is_reported_and_the_next_one_builds(self):
        self.failing = {2}
        artifact = self.run_build(self.community())
        failed = [e for e in self.events if e.get("kind") == "failed"]
        self.assertEqual([(f["repo"], f["tag"]) for f in failed], [("bad/bundle", "v1.2.0")])
        self.assertEqual(failed[0]["reason"], "some of its patches failed (Stockfish analysis)")
        self.assertEqual(artifact["community"]["repo"], "good/bundle")

    def test_a_patcher_that_exits_with_an_error_still_names_the_patch(self):
        def offline_patch(args, env=None, **options):
            inner = [str(a) for a in args][7:]
            if "patch" in inner and "list-patches" not in inner:
                box = Path(inner[inner.index("--out") + 1]).parent
                (box / "result.json").write_text(json.dumps({"success": False, "appliedPatches": ["A"], "failedPatches": [
                    {"name": "Stockfish analysis", "error": "UnknownHostException: github.com"}]}))
                raise BuildError("patch failed")
            return self.command(args, env, **options)
        made = self.community(mode="manual", candidates=[{"repo": "good/bundle"}])
        made.base.builder.command = offline_patch
        with self.assertRaises(community.CommunityError) as raised:
            self.run_build(made)
        self.assertIn("needs the network while patching", str(raised.exception))
        self.assertEqual(raised.exception.kind, "other_bundle")

    def test_when_every_bundle_fails_the_owner_is_told_to_choose_another(self):
        self.failing = {1, 2}
        with self.assertRaises(community.CommunityError) as raised:
            self.run_build(self.community())
        self.assertEqual(raised.exception.kind, "other_bundle")
        self.assertIn("No bundle built Discord", str(raised.exception))

    def test_bundles_that_failed_at_their_latest_release_are_skipped(self):
        artifact = self.run_build(self.community(skip={"bad/bundle": "v1.2.0"}))
        self.assertEqual(artifact["community"]["repo"], "good/bundle")
        self.assertFalse([u for u in self.downloads if "bad/bundle" in u])

    def test_a_manual_choice_builds_only_that_bundle(self):
        self.failing = {1}
        with self.assertRaises(community.CommunityError) as raised:
            self.run_build(self.community(mode="manual", candidates=[{"repo": "good/bundle"}]))
        self.assertIn("good/bundle didn't build", str(raised.exception))
        self.assertEqual(raised.exception.kind, "other_bundle")

    def pins(self, **change):
        return {"repo": "good/bundle", "repo_id": 1, "author": "good", "tag": "v1.1.0",
                "asset": "patches.mpp", "asset_sha256": android.digest(self.bundles[1]), "bundle_name": "bundle",
                "version": "351.11 - Stable", "version_code": 351011, "output_package": "com.discord",
                "output_version_code": 351011, "patches": [], **change}

    def test_a_pinned_rebuild_follows_its_pins(self):
        artifact = self.run_build(self.community(mode="pinned", candidates=[{"repo": "good/bundle"}], pins=self.pins(),
                                                 certificate=PUBLISHER.fingerprint))
        self.assertEqual(artifact["community"]["tag"], "v1.1.0")
        self.assertIn("https://api.github.com/repositories/1/releases/tags/v1.1.0", self.github_calls)
        self.assertFalse([e for e in self.events if e.get("kind") == "certificate"], "already pinned")

    def test_the_same_tag_with_other_bytes_is_a_hard_stop(self):
        with self.assertRaises(community.CommunityError) as raised:
            self.run_build(self.community(mode="pinned", candidates=[{"repo": "good/bundle"}],
                                          pins=self.pins(asset_sha256="0" * 64), certificate=PUBLISHER.fingerprint))
        self.assertTrue(raised.exception.blocked)
        self.assertIn("changed without a new release", str(raised.exception))
        self.assertFalse([c for c in self.commands if "patch" in c[0]])

    def test_a_renamed_repository_is_a_hard_stop(self):
        self.releases["moved/bundle"] = self.releases.pop("good/bundle")
        with self.assertRaises(community.CommunityError) as raised:
            self.run_build(self.community(mode="pinned", candidates=[{"repo": "good/bundle"}], pins=self.pins(),
                                          certificate=PUBLISHER.fingerprint))
        self.assertIn("moved or changed its name", str(raised.exception))

    def test_an_original_signed_by_another_publisher_is_refused(self):
        self.served[351011] = (STRANGER, "351.11 - Stable")
        with self.assertRaises(community.CommunityError) as raised:
            self.run_build(self.community(certificate=PUBLISHER.fingerprint))
        self.assertEqual(raised.exception.kind, "unavailable")
        self.assertTrue(raised.exception.blocked, "a hard stop")
        self.assertIn("signed by another publisher", str(raised.exception))
        self.assertFalse([c for c in self.commands if "patch" in c[0]])
        self.assertFalse([e for e in self.events if e.get("kind") == "failed"], "not the bundles' fault")

    def test_apkpure_is_only_trusted_once_play_pinned_the_certificate(self):
        made = self.community()
        made.play = lambda: self
        self.served = {}
        apkpure = []
        android.apkpure = lambda *args: apkpure.append(args) or []
        with self.assertRaises(community.CommunityError):
            self.run_build(made)
        self.assertEqual(apkpure, [], "no certificate pinned yet")
        made = self.community(certificate=PUBLISHER.fingerprint)
        with self.assertRaises(community.CommunityError):
            self.run_build(made)
        self.assertTrue(apkpure)

    def test_zero_applied_patches_is_a_failure(self):
        def nothing(args, env=None, **options):
            result = self.command(args, env, **options)
            inner = [str(a) for a in args][7:]
            if "patch" in inner:
                box = Path(inner[inner.index("--out") + 1]).parent
                (box / "result.json").write_text(json.dumps({"success": True, "appliedPatches": [], "failedPatches": []}))
            return result
        made = self.community(mode="manual", candidates=[{"repo": "good/bundle"}])
        made.base.builder.command = nothing
        with self.assertRaises(community.CommunityError) as raised:
            self.run_build(made)
        self.assertIn("no patches applied", str(raised.exception))

    def test_github_limits_ask_for_a_retry(self):
        made = self.community()
        made.http_github = lambda url, headers: (403, b"{}", {"x-ratelimit-remaining": "0"})
        with self.assertRaises(community.CommunityError) as raised:
            self.run_build(made)
        self.assertEqual(raised.exception.kind, "retry")

    def test_github_answers_are_cached_with_their_etag(self):
        made = self.community()
        replies = [(200, json.dumps({"id": 1, "full_name": "good/bundle"}).encode(), {"etag": '"abc"'}),
                   (304, b"", {})]
        sent = []

        def github(url, headers):
            sent.append(headers)
            return replies.pop(0)
        made.http_github = github
        self.assertEqual(made.github("/repos/good/bundle")["id"], 1)
        self.assertEqual(made.github("/repos/good/bundle")["full_name"], "good/bundle")
        self.assertEqual(sent[1]["If-None-Match"], '"abc"')


    def test_unused_bundles_and_lists_age_out(self):
        made = self.community()
        old, new = made.store / "bundles" / "old.mpp", made.store / "lists" / "new.txt"
        for path in (old, new):
            path.write_text("x")
        import os
        import time
        os.utime(old, (time.time() - 40 * 86400,) * 2)
        made.prune()
        self.assertFalse(old.exists())
        self.assertTrue(new.exists())


class PlayRetryTests(unittest.TestCase):
    def test_one_failed_split_download_is_retried_alone(self):
        originals = (android.download, android.http, android.RETRY_SECONDS)
        self.addCleanup(lambda: (setattr(android, "download", originals[0]), setattr(android, "http", originals[1]),
                                 setattr(android, "RETRY_SECONDS", originals[2])))
        android.RETRY_SECONDS = 0
        attempts = []

        def download(url, target, **_):
            attempts.append(url)
            if url.endswith("split") and attempts.count(url) == 1:
                raise android.SourceError("a download failed")
            Path(target).write_bytes(b"x")
            return "0" * 64
        android.download = download
        from test_android import SESSION, message
        replies = {
            "https://auroraoss.com/api/auth": (200, json.dumps(SESSION).encode()),
            android.PLAY + "purchase": (200, message((1, message((4, message((55, "t"))))))),
            android.PLAY + "delivery": (200, message((1, message((21, message((2, message(
                (1, 4096), (3, "https://play.example/base"),
                (15, message((1, "config.arm64_v8a"), (2, 2048), (5, "https://play.example/split"))),
            )))))))),
        }
        android.http = lambda url, **_: next(v for k, v in replies.items() if url.startswith(k))
        with tempfile.TemporaryDirectory() as folder:
            play = android.Play({"dispenser": "https://auroraoss.com/api/auth", "agent": "x"}, {}, Path(folder) / "s.json")
            files = play.fetch("com.example.app", 1, Path(folder))
        self.assertEqual(len(files), 2)
        self.assertEqual(attempts, ["https://play.example/base", "https://play.example/split", "https://play.example/split"])


if __name__ == "__main__":
    unittest.main()
