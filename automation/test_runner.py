from pathlib import Path
import hashlib
import importlib.util
import plistlib
import struct
import tempfile
import unittest
import zipfile
import runner
from unittest.mock import patch


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
            for key in ("name", "bundle_id", "tweak", "summary", "catalog", "limitations"):
                self.assertTrue(app.get(key), f"{app['id']} needs {key}")
            if app.get("native") or app.get("integration"):
                native = app.get("native") or app["integration"]
                self.assertRegex(native["dependency"]["sha256"], r"^[0-9a-f]{64}$")
                self.assertTrue(native["dependency"]["url"].startswith("https://"))
                self.assertTrue(native["source"])
                if app.get("native"):
                    self.assertRegex(native["version"], r"^\d+(?:\.\d+){0,3}$")
                    self.assertTrue(native["build"])
                else:
                    self.assertTrue(app["app_id"] and app["app_version"] and app["markers"])
                    self.assertEqual(app["extensions"][0]["bundle_id"], "com.google.keyboard.KeyboardExtension")
                self.assertEqual(app["packages"], [])
                continue
            self.assertTrue(app.get("app_id") and app.get("markers"))
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


class GboardInjectionTests(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        path = Path(__file__).parent.parent / "experiments/gboard-rambler/inject-gboard.py"
        spec = importlib.util.spec_from_file_location("gboard_injector", path)
        cls.injector = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(cls.injector)

    def image(self, *, encrypted=False, section_offset=512):
        segment = bytearray(152)
        struct.pack_into("<II", segment, 0, 0x19, len(segment))
        struct.pack_into("<I", segment, 64, 1)
        struct.pack_into("<QI", segment, 72 + 40, 256, section_offset)
        encryption = struct.pack("<IIIIII", 0x2c, 24, 0, 0, int(encrypted), 0)
        commands = segment + encryption
        header = struct.pack("<IIIIIIII", 0xfeedfacf, 0x100000c, 0, 2, 2, len(commands), 0, 0)
        data = bytearray((header + commands).ljust(1024, b"\0"))
        data[section_offset:section_offset + 256] = b"x" * 256
        return bytes(data)

    def test_injection_preserves_instructions_and_other_load_commands(self):
        source = self.image()
        library = "@executable_path/Frameworks/KiwiGboardRambler.dylib"
        result = self.injector.inject(source, library)
        self.assertEqual(len(result), len(source))
        self.assertEqual(result[32:208], source[32:208])
        self.assertEqual(result[512:], source[512:])
        self.assertEqual(runner.macho_info(result), (False, [library]))

    def test_encryption_padding_and_malformed_commands_prevent_injection(self):
        for data, error in [(self.image(encrypted=True), "encrypted"),
                            (self.image(section_offset=208), "padding"),
                            (b"unrecognized", "arm64")]:
            with self.subTest(error=error), self.assertRaisesRegex(ValueError, error):
                self.injector.inject(data, "@executable_path/Frameworks/KiwiGboardRambler.dylib")
        malformed = bytearray(self.image())
        struct.pack_into("<I", malformed, 36, 4)
        with self.assertRaisesRegex(ValueError, "length"):
            self.injector.inject(malformed, "client.dylib")

    def test_packaging_keeps_and_patches_the_keyboard_and_refuses_changed_images(self):
        with tempfile.TemporaryDirectory() as d:
            root = Path(d)
            source, target = root / "original.ipa", root / "patched.ipa"
            image = self.image()
            with zipfile.ZipFile(source, "w") as archive:
                for name, bundle in [("", "com.google.keyboard"), ("PlugIns/Gboard.appex/", "com.google.keyboard.KeyboardExtension")]:
                    archive.writestr("Payload/Gboard.app/" + name + "Info.plist", plistlib.dumps({
                        "CFBundleIdentifier": bundle, "CFBundleExecutable": "Gboard",
                        "CFBundleShortVersionString": "2.3.19", "CFBundleVersion": "2.3.19.444584732",
                        "CFBundleURLTypes": [{"CFBundleURLSchemes": ["gboard"]}],
                    }))
                for name in self.injector.IMAGES:
                    archive.writestr("Payload/Gboard.app/" + name, image)
                archive.writestr("Payload/Gboard.app/resource", b"preserved")
            library, caller, sample = (root / name for name in ("client.dylib", "caller.json", "sample.wav"))
            library.write_bytes(image)
            caller.write_text('{"fixture":true}')
            sample.write_bytes(b"synthetic")
            with self.assertRaisesRegex(ValueError, "differ from the inspected"):
                self.injector.package(source, library, caller, sample, target)
            pins = {name: hashlib.sha256(image).hexdigest() for name in self.injector.IMAGES}
            with patch.dict(self.injector.IMAGES, pins):
                self.injector.package(source, library, caller, sample, target)
            extension = {"path": "PlugIns/Gboard.appex", "bundle_id": "com.google.keyboard.KeyboardExtension", "markers": ["kiwigboardrambler"]}
            info = runner.inspect_ipa(target, "com.google.keyboard", decrypted=True, markers=extension["markers"], extensions=[extension])
            self.assertEqual(info["MinimumOSVersion"], "16.0")
            self.assertTrue(info["KiwiRequiredAppGroups"])
            self.assertEqual(info["CFBundleURLTypes"], [])
            with zipfile.ZipFile(target) as archive:
                self.assertEqual(archive.read("Payload/Gboard.app/resource"), b"preserved")
                nested = "Payload/Gboard.app/PlugIns/Gboard.appex/Gboard"
                self.assertEqual(runner.macho_info(archive.read(nested))[1], ["@executable_path/../../Frameworks/KiwiGboardRambler.dylib"])
            with self.assertRaisesRegex(runner.BuildError, "keyboard extension is missing"):
                runner.inspect_ipa(source, "com.google.keyboard", extensions=[{**extension, "path": "PlugIns/Missing.appex"}])
            with self.assertRaisesRegex(runner.BuildError, "does not load"):
                runner.inspect_ipa(source, "com.google.keyboard", extensions=[extension])
            with patch.dict(self.injector.IMAGES, pins), self.assertRaises(FileExistsError):
                self.injector.package(source, library, caller, sample, source)
            self.assertEqual(list(root.glob('.gboard-*')), [])
            with zipfile.ZipFile(source) as archive:
                self.assertEqual(archive.read("Payload/Gboard.app/Gboard"), image)


class NativeBuildTests(unittest.TestCase):
    toolchain = {"sdk": "27.0", "compiler": "Fixture Swift compiler"}

    def sources(self, builder):
        manifest = builder.root / "native/rambler.json"
        manifest.parent.mkdir()
        manifest.write_bytes(runner.canonical({"build.sh": "fixture source"}))
        return manifest

    def plan(self, builder):
        with patch.object(builder, "native_toolchain", return_value=self.toolchain), \
                patch.object(builder, "doctor", side_effect=AssertionError("App Store setup was used")), \
                patch.object(builder, "ipa", side_effect=AssertionError("App Store was contacted")), \
                patch.object(runner, "emit") as emit:
            builder.plan(builder.app("rambler"), {"tweaks": []})
        return emit.call_args.kwargs["plan"]

    def test_native_plan_uses_bundled_sources_without_app_store_enrollment(self):
        with tempfile.TemporaryDirectory() as d:
            builder = runner.Builder(d)
            manifest = self.sources(builder)
            plan = self.plan(builder)
            self.assertEqual(plan["version"], "0.1.0")
            self.assertEqual(plan["native_source_sha256"], runner.digest(manifest))
            self.assertTrue(plan["external_id"].startswith("native-"))
            self.assertEqual(plan["key"], runner.recipe_key({k: v for k, v in plan.items() if k != "key"}))
            job = builder.root / "jobs/test"
            job.mkdir()
            manifest.write_bytes(runner.canonical({"build.sh": "changed after planning"}))
            with patch.object(builder, "native_toolchain", return_value=self.toolchain), \
                    patch.object(builder, "command", side_effect=AssertionError("Build started with changed sources")), \
                    self.assertRaisesRegex(runner.BuildError, "inputs changed"):
                builder.build(builder.app("rambler"), plan, 1, job)

    def test_native_source_manifest_cannot_write_outside_job(self):
        with tempfile.TemporaryDirectory() as d:
            builder = runner.Builder(d)
            manifest = self.sources(builder)
            for name in ("../outside", "/absolute", "Sources/../../outside", "C:\\outside", "."):
                with self.subTest(name=name):
                    manifest.write_bytes(runner.canonical({name: "source"}))
                    with self.assertRaisesRegex(runner.BuildError, "inside"):
                        builder.native_sources(builder.app("rambler"))

    def test_native_dependency_is_checksum_verified_before_use_and_on_cache_hits(self):
        with tempfile.TemporaryDirectory() as d:
            builder = runner.Builder(d)
            data = b"pinned APK fixture"
            sha = runner.hashlib.sha256(data).hexdigest()
            app = {"native": {"dependency": {"url": "https://example.test/input.apk", "sha256": sha}}}
            cache = builder.tools / f"native-inputs/{sha}.apk"

            def download(argv, **_kwargs):
                Path(argv[argv.index("--output") + 1]).write_bytes(data)

            def corrupt_download(argv, **_kwargs):
                Path(argv[argv.index("--output") + 1]).write_bytes(b"changed upstream bytes")

            with patch.object(builder, "tool", return_value="curl"), patch.object(runner, "emit"):
                with patch.object(builder, "command", side_effect=corrupt_download), \
                        self.assertRaisesRegex(runner.BuildError, "SHA-256"):
                    builder.native_dependency(app)
                self.assertFalse(cache.exists())
                self.assertFalse(cache.with_suffix(".partial").exists())
                with patch.object(builder, "command", side_effect=download) as command:
                    self.assertEqual(builder.native_dependency(app), cache)
                    self.assertEqual(builder.native_dependency(app), cache)
                    self.assertEqual(command.call_count, 1)
                    cache.write_bytes(b"corrupt cached APK")
                    builder.native_dependency(app)
                    self.assertEqual(command.call_count, 2)
                self.assertEqual(cache.read_bytes(), data)

    def test_native_build_only_emits_an_artifact_after_real_correction_result(self):
        valid = {"text": "Let us meet on Wednesday at 2:00.", "voiceEditing": True, "completed": True}
        results = [valid, {**valid, "voiceEditing": False}, {**valid, "completed": False},
                   {**valid, "text": "Let us meet on Tuesday, actually Wednesday."}]
        for result in results:
            with self.subTest(result=result), tempfile.TemporaryDirectory() as d:
                builder = runner.Builder(d)
                self.sources(builder)
                plan = self.plan(builder)
                app = builder.app("rambler")
                job = builder.root / "jobs/test"
                job.mkdir()
                private = "fixture-caller-value-never-published-in-logs"
                calls = []

                def command(argv, **_kwargs):
                    args = [str(a) for a in argv]
                    calls.append(args)
                    if args[0].endswith("rambler-cli"):
                        return runner.json.dumps(result)
                    if args[1].endswith("extract-caller.py"):
                        Path(args[3]).write_text(private)
                    elif args[1].endswith("make-sample.sh"):
                        Path(args[2]).write_bytes(b"synthetic speech fixture")
                    elif args[1].endswith("build.sh"):
                        output = Path(args[3])
                        output.mkdir(exist_ok=True)
                        if args[2] == "device":
                            with zipfile.ZipFile(output / "RamblerPoC.ipa", "w") as archive:
                                archive.writestr("Payload/RamblerPoC.app/Info.plist", plistlib.dumps({
                                    "CFBundleIdentifier": app["bundle_id"], "CFBundleExecutable": "RamblerPoC",
                                    "CFBundleShortVersionString": "0.1.0", "CFBundleVersion": "1", "MinimumOSVersion": "16.0",
                                }))
                                archive.writestr("Payload/RamblerPoC.app/RamblerPoC", executable())
                                archive.writestr("Payload/RamblerPoC.app/caller.json", private)
                    return ""

                with patch.object(builder, "native_toolchain", return_value=self.toolchain), \
                        patch.object(builder, "native_dependency", return_value=Path(d) / "pinned.apk"), \
                        patch.object(builder, "command", side_effect=command), patch.object(runner, "emit") as emit:
                    if result != valid:
                        with self.assertRaisesRegex(runner.BuildError, "not published"):
                            builder.build(app, plan, 7, job)
                        self.assertFalse(any("device" in args for args in calls))
                        self.assertEqual(list((builder.root / "artifacts").iterdir()), [])
                        self.assertFalse(any(c.args[0] == "artifact" for c in emit.call_args_list))
                        continue
                    builder.build(app, plan, 7, job)
                event = emit.call_args.kwargs
                artifact = builder.root / "artifacts" / f"{event['sha256']}.ipa"
                info = runner.inspect_ipa(artifact, app["bundle_id"], decrypted=True)
                self.assertEqual(info["KiwiBuildRevision"], 7)
                self.assertEqual(event["build"], "1")
                provenance = artifact.with_suffix(".json").read_text()
                self.assertTrue(runner.json.loads(provenance)["speech_service_verified"])
                self.assertFalse(runner.json.loads(provenance)["runtime_tested"])
                self.assertNotIn(private, provenance)
                self.assertNotIn(private, str(emit.call_args_list))


if __name__ == "__main__":
    unittest.main()
