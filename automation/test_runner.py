from pathlib import Path
import plistlib
import struct
import tempfile
import unittest
import zipfile
import runner


def executable(encrypted=False, library="@executable_path/Frameworks/YouMod.dylib"):
    name = library.encode() + b"\0"
    size = (24 + len(name) + 7) // 8 * 8
    dylib = struct.pack("<IIIIII", 0xc, size, 24, 0, 0, 0) + name
    dylib = dylib.ljust(size, b"\0")
    encryption = struct.pack("<IIIIII", 0x2c, 24, 0, 0, int(encrypted), 0)
    return struct.pack("<IIIIIIII", 0xfeedfacf, 0x100000c, 0, 2, 2, len(dylib) + 24, 0, 0) + dylib + encryption


def ipa(path, *, encrypted=False, library="@executable_path/Frameworks/YouMod.dylib", extra=None):
    with zipfile.ZipFile(path, "w") as z:
        z.writestr("Payload/YouTube.app/Info.plist", plistlib.dumps({
            "CFBundleIdentifier": "com.google.ios.youtube", "CFBundleExecutable": "YouTube",
            "CFBundleShortVersionString": "21.44.1", "CFBundleVersion": "21.44.1",
            "MinimumOSVersion": "17.0",
        }))
        z.writestr("Payload/YouTube.app/YouTube", executable(encrypted, library))
        z.writestr("Payload/YouTube.app/Frameworks/YouMod.dylib", executable(False))
        if extra:
            z.writestr(*extra)


class RunnerTests(unittest.TestCase):
    def test_encrypted_nested_extension_prevents_publication(self):
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / "app.ipa"
            ipa(p, extra=("Payload/YouTube.app/PlugIns/Upload.appex/Upload", executable(True)))
            with self.assertRaisesRegex(runner.BuildError, "remains encrypted"):
                runner.inspect_ipa(p, "com.google.ios.youtube", decrypted=True, markers=["youmod"])
            # Extensions that injection removes need not be decrypted beforehand.
            runner.inspect_ipa(p, "com.google.ios.youtube", decrypted=True, removable=True)

    def test_injection_must_be_loaded_by_the_main_executable(self):
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / "app.ipa"
            ipa(p, library="@executable_path/Unrelated.dylib")
            with self.assertRaisesRegex(runner.BuildError, "load commands"):
                runner.inspect_ipa(p, "com.google.ios.youtube", decrypted=True, markers=["youmod"])
            ipa(p)
            self.assertEqual(runner.inspect_ipa(p, "com.google.ios.youtube", decrypted=True, markers=["youmod"])["MinimumOSVersion"], "17.0")
            with self.assertRaisesRegex(runner.BuildError, "unexpected bundle identifier"):
                runner.inspect_ipa(p, "tv.twitch")

    def test_zip_traversal_and_escaping_symlinks_are_rejected(self):
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / "app.ipa"
            ipa(p, extra=("Payload/../../outside", b"bad"))
            with self.assertRaisesRegex(runner.BuildError, "unsafe"):
                runner.inspect_ipa(p, "com.google.ios.youtube")
            link = zipfile.ZipInfo("Payload/YouTube.app/link")
            link.external_attr = 0o120777 << 16
            ipa(p, extra=(link, "../../../../etc/passwd"))
            with self.assertRaisesRegex(runner.BuildError, "symbolic link"):
                runner.inspect_ipa(p, "com.google.ios.youtube")

    def test_fat_executables_check_every_slice(self):
        a, b = executable(False), executable(True)
        offset_a = 48
        offset_b = offset_a + len(a)
        fat = struct.pack(">II", 0xcafebabe, 2)
        fat += struct.pack(">IIIII", 0x100000c, 0, offset_a, len(a), 0)
        fat += struct.pack(">IIIII", 0x100000c, 0, offset_b, len(b), 0)
        self.assertTrue(runner.macho_info(fat + a + b)[0])
        with self.assertRaises(runner.BuildError):
            runner.macho_info(fat)

    def test_new_patch_revision_preserves_both_youtube_versions_and_resources(self):
        with tempfile.TemporaryDirectory() as d:
            source, target = Path(d) / "in.ipa", Path(d) / "out.ipa"
            ipa(source, extra=("Payload/YouTube.app/resource", b"resource bytes"))
            runner.stamp_revision(source, target, 102)
            info = runner.inspect_ipa(target, "com.google.ios.youtube", decrypted=True, markers=["youmod"])
            self.assertEqual(info["CFBundleVersion"], "21.44.1")
            self.assertEqual(info["KiwiBuildRevision"], 102)
            self.assertEqual(info["CFBundleShortVersionString"], "21.44.1")
            with zipfile.ZipFile(target) as z:
                self.assertEqual(z.read("Payload/YouTube.app/resource"), b"resource bytes")

    def test_recipe_identity_changes_with_base_and_patch_configuration(self):
        p = {"external_id": "123", "tweaks": ["youpip"], "recipe": {"commit": "aaa"}}
        self.assertNotEqual(runner.recipe_key(p), runner.recipe_key({**p, "external_id": "124"}))
        self.assertNotEqual(runner.recipe_key(p), runner.recipe_key({**p, "tweaks": []}))
        self.assertNotEqual(runner.recipe_key(p), runner.recipe_key({**p, "recipe": {"commit": "bbb"}}))

    def test_apple_results_are_selected_without_forwarding_account_data(self):
        result = runner.read_json_output('{"level":"info","message":"authenticating"}\n{"success":true,"displayVersion":"21.44.1"}\n')
        self.assertEqual(result["displayVersion"], "21.44.1")
        with self.assertRaisesRegex(runner.BuildError, "Sign in"):
            runner.read_json_output('{"error":"private account response"}')

    def test_tool_timeout_does_not_leave_a_running_child(self):
        import os
        import sys
        with tempfile.TemporaryDirectory() as d:
            p = Path(d) / "pid"
            code = f"import os,time;open({str(p)!r},'w').write(str(os.getpid()));time.sleep(30)"
            with self.assertRaises(runner.BuildError):
                runner.run([sys.executable, "-c", code], timeout=0.3)
            with self.assertRaises(ProcessLookupError):
                os.kill(int(p.read_text()), 0)

    def test_plan_uses_apples_current_build_rather_than_history_order(self):
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as d:
            builder = runner.Builder(d)
            (builder.tools / "setup.json").write_text('{}')
            answers = [
                {"externalVersionIdentifiers": ["123", "999"], "latestExternalVersionID": "123"},
                {"displayVersion": "21.44.1"},
            ]
            with patch.object(builder, "doctor", return_value={"ready": True, "checks": []}), \
                 patch.object(builder, "ipa", side_effect=answers) as apple, patch.object(runner, "emit") as emit:
                builder.plan(builder.app("youtube"), {"tweaks": []})
            self.assertEqual(apple.call_args_list[1].args[0][4], "123")
            plan = emit.call_args.kwargs["plan"]
            self.assertEqual(plan["external_id"], "123")
            self.assertEqual(plan["key"], runner.recipe_key({k: v for k, v in plan.items() if k != "key"}))

    def test_native_enrollment_preserves_original_binary_and_license(self):
        with tempfile.TemporaryDirectory() as d:
            wrapper = Path(d) / "Sample.app"
            native = wrapper / "Wrapper/Sample.app"
            (native / "SC_Info").mkdir(parents=True)
            (native / "Info.plist").write_bytes(plistlib.dumps({"CFBundleExecutable": "Sample"}))
            original = executable(True)
            (native / "Sample").write_bytes(original)
            (native / "SC_Info/Sample.sinf").write_bytes(b"private original license")
            # Packaging must preserve links rather than follow them into other data.
            (native / "link").symlink_to("SC_Info/Sample.sinf")
            target = Path(d) / "enrollment.ipa"
            runner.installed_ipa(wrapper, target)
            with zipfile.ZipFile(target) as archive:
                self.assertEqual(archive.read("Payload/Sample.app/Sample"), original)
                self.assertEqual(archive.read("Payload/Sample.app/SC_Info/Sample.sinf"), b"private original license")
                self.assertEqual(archive.read("Payload/Sample.app/link"), b"SC_Info/Sample.sinf")
                self.assertTrue(runner.macho_info(archive.read("Payload/Sample.app/Sample"))[0])

    def test_recipes_are_complete_and_pinned(self):
        recipes = runner.json.loads(runner.RECIPES_PATH.read_text())
        pin = runner.re.compile(r"[0-9a-f]{40}")
        specs = list(recipes["toolchain"]["repositories"].values()) + list(recipes["toolchain"]["headers"].values())
        ids = [app["id"] for app in recipes["apps"]]
        self.assertEqual(len(ids), len(set(ids)))
        self.assertIn("youtube", ids)
        for app in recipes["apps"]:
            for key in ("name", "app_id", "bundle_id", "tweak", "summary", "markers", "catalog", "limitations"):
                self.assertTrue(app.get(key), f"{app['id']} needs {key}")
            packages = [p["id"] for p in app["packages"]]
            self.assertEqual(len(packages), len(set(packages)))
            self.assertTrue(any(not p.get("optional") for p in app["packages"]))
            for package in app["packages"]:
                if package.get("optional"):
                    self.assertTrue(package.get("name") and package.get("description"), package["id"])
            specs += [spec for spec, _ in runner.Builder.app_sources(None, app)]
            if app.get("names"):
                self.assertIn("{name}", app["catalog"]["name"])
        for spec in specs:
            self.assertRegex(spec[0], runner.REPO)
            self.assertRegex(spec[1], pin)

    def test_build_patches_must_match_exactly_once_inside_the_source(self):
        with tempfile.TemporaryDirectory() as d:
            source = Path(d) / "src"
            source.mkdir()
            (source / "Makefile").write_text("ifeq ($(SIDELOADED),1)\nMODULES = jailed\nendif\nTWEAK_NAME = Demo\n")
            patch = {"file": "Makefile", "find": "ifeq ($(SIDELOADED),1)\nMODULES = jailed\nendif\n", "replace": ""}
            runner.apply_patches(source, [patch], "demo")
            self.assertEqual((source / "Makefile").read_text(), "TWEAK_NAME = Demo\n")
            with self.assertRaisesRegex(runner.BuildError, "no longer matches"):
                runner.apply_patches(source, [patch], "demo")
            (Path(d) / "outside").write_text("x")
            with self.assertRaisesRegex(runner.BuildError, "inside"):
                runner.apply_patches(source, [{"file": "../outside", "find": "x", "replace": "y"}], "demo")

    def test_pinned_sources_are_fetched_once(self):
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as d:
            builder = runner.Builder(d)
            spec = ["Owner/Repo", "a" * 40]
            def fetch(_spec, target, recursive=False):
                target.mkdir(parents=True)
                (target / "file").write_text("pinned")
                return target
            with patch.object(builder, "checkout", side_effect=fetch) as checkout, patch.object(runner, "emit"):
                first = builder.source(spec, submodules=True)
                second = builder.source(spec, submodules=True)
            self.assertEqual(first, second)
            self.assertEqual(checkout.call_count, 1)
            self.assertEqual((first / "file").read_text(), "pinned")
            # An interrupted fetch leaves no complete marker and is fetched again.
            first.with_name(first.name + ".complete").unlink()
            with patch.object(builder, "checkout", side_effect=lambda s, t, recursive=False: fetch(s, t)) as checkout, \
                    patch.object(runner, "emit"):
                builder.source(spec, submodules=True)
            self.assertEqual(checkout.call_count, 1)
            with self.assertRaisesRegex(runner.BuildError, "complete Git commit"):
                builder.source(["Owner/Repo", "main"])

    def test_pinned_app_versions_use_the_matching_apple_build_and_remember_lookups(self):
        from unittest.mock import patch
        with tempfile.TemporaryDirectory() as d:
            builder = runner.Builder(d)
            app = {**builder.app("x"), "app_version": "12.38.1"}
            versions = {"300": "12.39", "200": "12.38.1", "100": "12.37"}
            with patch.object(builder, "version_of", side_effect=lambda _app, ext: versions[ext]) as lookup:
                self.assertEqual(builder.pinned_build(app, ["100", "200", "300"], "300"), "200")
                self.assertEqual(lookup.call_count, 2)
                self.assertEqual(builder.pinned_build(app, ["100", "200", "300"], "300"), "200")
                self.assertEqual(lookup.call_count, 2)
                with self.assertRaisesRegex(runner.BuildError, "no longer offers"):
                    builder.pinned_build({**app, "app_version": "11.0"}, ["100", "200", "300"], "300")

    def test_options_are_validated_against_the_recipe(self):
        with tempfile.TemporaryDirectory() as d:
            builder = runner.Builder(d)
            self.assertEqual(builder.options(builder.app("x"), {"tweaks": [], "name": "Twitter"}), ([], "Twitter"))
            self.assertEqual(builder.options(builder.app("youtube"), {"tweaks": ["ryd", "ryd"], "name": "X"}), (["ryd"], None))
            with self.assertRaisesRegex(runner.BuildError, "name"):
                builder.options(builder.app("x"), {"tweaks": [], "name": "Bird"})
            with self.assertRaisesRegex(runner.BuildError, "tweak"):
                builder.options(builder.app("twitch"), {"tweaks": ["ryd"]})
            with self.assertRaisesRegex(runner.BuildError, "Unknown app"):
                builder.app("instagram")

    def test_bundle_products_have_stable_tree_digests(self):
        with tempfile.TemporaryDirectory() as d:
            bundle = Path(d) / "Demo.bundle"
            (bundle / "en.lproj").mkdir(parents=True)
            (bundle / "en.lproj/Localizable.strings").write_text("a")
            first = runner.tree_digest(bundle)
            self.assertEqual(first, runner.tree_digest(bundle))
            (bundle / "en.lproj/Localizable.strings").write_text("b")
            self.assertNotEqual(first, runner.tree_digest(bundle))


if __name__ == "__main__":
    unittest.main()
