from contextlib import redirect_stdout
import hashlib
import io
import json
from pathlib import Path
import stat
import struct
import tempfile
import unittest
import zipfile

import android
import apk
from runner import BuildError, Builder

P256 = bytes.fromhex("2a8648ce3d030107")
CURVE = apk.CURVES[P256]
BASE = (CURVE[3], CURVE[4])


def der(tag, body):
    size = len(body)
    head = bytes([size]) if size < 0x80 else bytes([0x80 | (size.bit_length() + 7) // 8]) + size.to_bytes((size.bit_length() + 7) // 8, "big")
    return bytes([tag]) + head + body


def integer(value):
    return der(2, value.to_bytes(value.bit_length() // 8 + 1, "big"))


class Key:
    """A P-256 signing key with a certificate, as a publisher would hold."""

    def __init__(self, secret):
        self.secret = secret
        x, y = apk._multiply(CURVE, secret, BASE)
        point = b"\x04" + x.to_bytes(32, "big") + y.to_bytes(32, "big")
        self.spki = der(0x30, der(0x30, der(6, apk.EC) + der(6, P256)) + der(3, b"\0" + point))
        name = der(0x30, der(0x31, der(0x30, der(6, bytes.fromhex("550403")) + der(0x0C, b"Test"))))
        algorithm = der(0x30, der(6, bytes.fromhex("2a8648ce3d040302")))
        validity = der(0x30, der(0x17, b"250101000000Z") + der(0x17, b"350101000000Z"))
        tbs = der(0x30, der(0xA0, integer(2)) + integer(secret) + algorithm + name + validity + name + self.spki)
        # Android never checks the certificate's own signature, and neither does apk.py.
        self.certificate = der(0x30, tbs + algorithm + der(3, bytes(9)))
        self.fingerprint = hashlib.sha256(self.certificate).hexdigest()

    def sign(self, message):
        order = CURVE[5]
        nonce = int.from_bytes(hashlib.sha256(b"nonce" + message).digest(), "big") % (order - 1) + 1
        r = apk._multiply(CURVE, nonce, BASE)[0] % order
        value = int.from_bytes(hashlib.sha256(message).digest(), "big")
        return der(0x30, integer(r) + integer(pow(nonce, -1, order) * (value + r * self.secret) % order))


PUBLISHER, STRANGER = Key(0x1234567890ABCDEF), Key(0xFEDCBA0987654321)


def manifest(package, code, version, split=None):
    """A compiled AndroidManifest.xml carrying only what Kiwi reads."""
    strings = ["versionCode", "versionName", "minSdkVersion", "package", "split", "manifest", "uses-sdk", package, version, split or ""]
    data, offsets = b"", []
    for value in strings:
        offsets.append(len(data))
        data += struct.pack("<H", len(value)) + value.encode("utf-16-le") + b"\0\0"
    data += bytes(-len(data) % 4)
    start = 28 + 4 * len(strings)
    pool = struct.pack("<HHIIIIII", 1, 28, start + len(data), len(strings), 0, 0, start, 0)
    pool += struct.pack(f"<{len(offsets)}I", *offsets) + data
    resources = struct.pack("<HHIIII", 0x0180, 8, 20, 0x0101021B, 0x0101021C, 0x0101020C)

    def element(name, attributes):
        body = struct.pack("<IIHHHHHH", 0xFFFFFFFF, name, 20, 20, len(attributes), 0, 0, 0)
        for key, kind, value in attributes:
            body += struct.pack("<IIIHBBI", 0xFFFFFFFF, key, value if kind == 3 else 0xFFFFFFFF, 8, 0, kind, value)
        return struct.pack("<HHIII", 0x0102, 16, 16 + len(body), 1, 0xFFFFFFFF) + body

    def end(name):
        return struct.pack("<HHIIIII", 0x0103, 16, 24, 1, 0xFFFFFFFF, 0xFFFFFFFF, name)

    attributes = [(0, 0x10, code), (1, 3, 8), (3, 3, 7)] + ([(4, 3, 9)] if split else [])
    body = pool + resources + element(5, attributes) + element(6, [(2, 0x10, 31)]) + end(6) + end(5)
    return struct.pack("<HHI", 3, 8, 8 + len(body)) + body


def lp(data):
    return struct.pack("<I", len(data)) + data


def package(path, key=PUBLISHER, *, name="com.example.app", code=42, version="1.2.3", split=None, abi="arm64-v8a",
            scheme=2, signing=None, claimed=None):
    """Writes an APK signed the way `apksigner` does (scheme v2 or v3)."""
    plain = io.BytesIO()
    with zipfile.ZipFile(plain, "w") as archive:
        archive.writestr("AndroidManifest.xml", manifest(name, code, version, split))
        if abi:
            archive.writestr(f"lib/{abi}/libapp.so", b"native code")
    data = plain.getvalue()
    end = data.rfind(b"PK\x05\x06")
    directory = struct.unpack_from("<I", data, end + 16)[0]
    parts = (data[:directory], data[directory:end], data[end:])
    chunks = [hashlib.sha256(b"\xa5" + struct.pack("<I", len(part)) + part).digest() for part in parts]
    content = hashlib.sha256(b"\x5a" + struct.pack("<I", len(chunks)) + b"".join(chunks)).digest()
    signed = lp(lp(struct.pack("<I", 0x0201) + lp(content))) + lp(lp(key.certificate)) + lp(b"")
    signature = (signing or key).sign(signed)
    signer = lp(signed) + (struct.pack("<II", 24, 0x7FFFFFFF) if scheme == 3 else b"")
    signer += lp(lp(struct.pack("<I", 0x0201) + lp(signature))) + lp((claimed or key).spki)
    value = lp(lp(signer))
    pair = struct.pack("<QI", len(value) + 4, {2: 0x7109871A, 3: 0xF05368C0}[scheme]) + value
    block = struct.pack("<Q", len(pair) + 24) + pair + struct.pack("<Q", len(pair) + 24) + apk.MAGIC
    path.write_bytes(parts[0] + block + parts[1] + parts[2][:16] + struct.pack("<I", directory + len(block)) + parts[2][20:])
    return path


def varint(value):
    out = b""
    while True:
        out += bytes([value & 0x7F | (0x80 if value > 0x7F else 0)])
        value >>= 7
        if not value:
            return out


def message(*fields):
    """A protobuf message from (field number, int | str | bytes) pairs."""
    out = b""
    for number, value in fields:
        if isinstance(value, int):
            out += varint(number << 3) + varint(value)
        else:
            value = value.encode() if isinstance(value, str) else value
            out += varint(number << 3 | 2) + varint(len(value)) + value
    return out


APP = {"id": "android-example", "name": "Example", "publisher": "Example Inc", "original_package": "com.example.app",
       "bundle_id": "com.example.app", "certificate": PUBLISHER.fingerprint, "app_version": "1.2.3", "version_code": 42,
       "packages": [{"id": "core", "patches": []}, {"id": "extra", "optional": True, "patches": ["Extra"]}]}
PLAN = {"external_id": "42", "version": "1.2.3"}
SESSION = {"authToken": "token-1", "gsfId": "device-1", "deviceInfoProvider": {"userAgentString": "Finsky", "mccMnc": "20408"}}


class ApkTests(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)
        self.path = Path(self.dir.name) / "app.apk"

    def test_reports_the_manifest_and_who_signed_every_byte(self):
        for scheme in (2, 3):
            facts = apk.inspect(package(self.path, scheme=scheme))
            self.assertEqual(facts, {
                "package": "com.example.app", "version": "1.2.3", "version_code": 42, "min_sdk": 31,
                "certificates": [PUBLISHER.fingerprint], "schemes": [scheme], "abis": ["arm64-v8a"],
                "size": self.path.stat().st_size,
            })
        self.assertEqual(apk.inspect(package(self.path, split="config.arm64_v8a"))["split"], "config.arm64_v8a")

    def test_a_changed_package_is_rejected(self):
        data = bytearray(package(self.path).read_bytes())
        at = data.index(b"native code")
        data[at] ^= 1
        self.path.write_bytes(data)
        with self.assertRaisesRegex(apk.ApkError, "changed after it was signed"):
            apk.inspect(self.path)
        # The central directory and the end record are signed as well.
        data = bytearray(package(self.path).read_bytes())
        data[data.rindex(b"lib/arm64-v8a/libapp.so") + 4] ^= 1
        self.path.write_bytes(data)
        with self.assertRaisesRegex(apk.ApkError, "changed after it was signed"):
            apk.inspect(self.path)

    def test_a_signature_must_come_from_the_certificate_it_names(self):
        package(self.path, signing=STRANGER)
        with self.assertRaisesRegex(apk.ApkError, "does not verify"):
            apk.inspect(self.path)
        # A stranger's valid signature cannot borrow the publisher's certificate.
        package(self.path, signing=STRANGER, claimed=STRANGER)
        with self.assertRaisesRegex(apk.ApkError, "does not match its certificate"):
            apk.inspect(self.path)
        self.assertEqual(apk.inspect(package(self.path, STRANGER))["certificates"], [STRANGER.fingerprint])

    def test_unsigned_and_foreign_files_are_rejected(self):
        with zipfile.ZipFile(self.path, "w") as archive:
            archive.writestr("AndroidManifest.xml", manifest("com.example.app", 42, "1.2.3"))
        with self.assertRaisesRegex(apk.ApkError, "no APK signature"):
            apk.inspect(self.path)
        self.path.write_bytes(b"not a package at all" * 8)
        with self.assertRaisesRegex(apk.ApkError, "not a ZIP archive"):
            apk.inspect(self.path)
        data = package(self.path).read_bytes()
        self.path.write_bytes(data[:-40])
        with self.assertRaises(apk.ApkError):
            apk.inspect(self.path)


class AndroidTests(unittest.TestCase):
    def setUp(self):
        self.dir = tempfile.TemporaryDirectory()
        self.addCleanup(self.dir.cleanup)
        self.root = Path(self.dir.name)
        self.builder = Builder(self.root / "data")
        self.builder.log = io.BytesIO()
        self.android = android.Android(self.builder)
        self.files = self.root / "files"
        self.files.mkdir()
        self.job = self.root / "job"
        self.job.mkdir()
        self.calls = []
        originals = (android.http, android.download)
        self.addCleanup(lambda: (setattr(android, "http", originals[0]), setattr(android, "download", originals[1])))
        android.http = self.offline
        android.download = self.offline

    def offline(self, url, *_args, **_kwargs):
        raise AssertionError(f"unexpected request to {url}")

    def apk(self, file, **options):
        return package(self.files / file, **options)

    def serve(self, replies, downloads):
        """Answers Kiwi's requests from memory and records them."""
        def http(url, *, headers=None, body=None, **_):
            self.calls.append((url, headers or {}, body))
            for prefix, reply in replies.items():
                if url.startswith(prefix):
                    return reply(len([c for c in self.calls if c[0].startswith(prefix)])) if callable(reply) else reply
            raise android.SourceError("it could not be reached")

        def download(url, target, **_):
            self.calls.append((url, {}, None))
            Path(target).write_bytes(downloads[url].read_bytes())
            return android.digest(target)

        android.http, android.download = http, download

    def play(self):
        return {
            "https://auroraoss.com/api/auth": (200, json.dumps(SESSION).encode()),
            android.PLAY + "purchase": (200, message((1, message((4, message((55, "delivery-token"))))))),
            android.PLAY + "delivery": (200, message((1, message((21, message((2, message(
                (1, 4096), (3, "https://play.example/base"),
                (15, message((1, "config.arm64_v8a"), (2, 2048), (5, "https://play.example/split"))),
            )))))))),
        }

    def test_recipes_pin_everything_a_build_downloads(self):
        recipes = self.android.recipes
        tools = [recipes["toolchain"]["patcher"], recipes["toolchain"]["patches"], *recipes["toolchain"]["java"]["archives"].values()]
        for tool in tools:
            self.assertTrue(tool["url"].startswith("https://"))
            self.assertRegex(tool["sha256"], r"^[0-9a-f]{64}$")
            self.assertGreater(tool["size"], 0)
        self.assertLessEqual(set(recipes["toolchain"]["sources"]), {"play", "apkpure"})
        self.assertEqual([a["id"] for a in recipes["apps"]], ["android-youtube", "android-youtube-music", "android-reddit"])
        for app in recipes["apps"]:
            self.assertEqual(app["platform"], "android")
            self.assertRegex(app["certificate"], r"^[0-9a-f]{64}$")
            self.assertIsInstance(app["version_code"], int)
            self.assertTrue(all(p["patches"] for p in app["packages"] if p.get("optional")))
        # The apps keep their publishers' name and icon.
        self.assertEqual([a.get("disabled_patches") for a in recipes["apps"]],
                         [["Custom branding"], ["Custom branding"], None])

    def test_an_original_must_be_the_publishers_own_planned_version(self):
        base, split = self.apk("base.apk"), self.apk("split.apk", split="config.arm64_v8a", abi=None)
        self.android.verified(APP, PLAN, [base, split])
        for files, reason in (
            ([self.apk("a.apk", key=STRANGER)], "not signed by Example Inc"),
            ([self.apk("b.apk", name="com.example.other")], "a different app"),
            ([self.apk("c.apk", code=43)], "a different version"),
            ([self.apk("d.apk", version="1.2.4")], "a different version"),
            ([base, self.apk("e.apk", split="config.fr", code=41, abi=None)], "a different version"),
            ([split], "an incomplete app"),
            ([base, base], "an incomplete app"),
            ([self.apk("f.apk", abi="x86_64")], "a different processor"),
        ):
            with self.assertRaisesRegex(android.SourceError, reason):
                self.android.verified(APP, PLAN, files)

    def test_google_play_serves_the_pinned_version_to_an_anonymous_session(self):
        base, split = self.apk("base.apk"), self.apk("split.apk", split="config.arm64_v8a", abi=None)
        self.serve(self.play(), {"https://play.example/base": base, "https://play.example/split": split})
        files, source = self.android.original(APP, PLAN, self.job)
        self.assertEqual(source, "Google Play")
        self.assertEqual([apk.inspect(f).get("split") for f in files], [None, "config.arm64_v8a"])
        dispenser, purchase, delivery = self.calls[0], self.calls[1], self.calls[2]
        # The dispenser learns the device profile and nothing about the owner.
        self.assertEqual(json.loads(dispenser[2]), self.android.recipes["device"])
        self.assertIn("doc=com.example.app&vc=42", purchase[0])
        self.assertEqual(purchase[1]["Authorization"], "Bearer token-1")
        self.assertIn("dtok=delivery-token", delivery[0])
        session = self.android.private / "play-session.json"
        self.assertEqual(stat.S_IMODE(session.stat().st_mode), 0o600)
        # The verified original is kept: the next build needs no network at all.
        android.http = android.download = self.offline
        again, source = self.android.original(APP, PLAN, self.job)
        self.assertEqual((again, source), (files, "Google Play"))

    def test_an_expired_play_session_is_renewed_once(self):
        replies = self.play()
        replies[android.PLAY + "purchase"] = lambda attempt: (401, b"") if attempt == 1 else self.play()[android.PLAY + "purchase"]
        self.serve(replies, {"https://play.example/base": self.apk("base.apk"),
                             "https://play.example/split": self.apk("split.apk", split="config.arm64_v8a", abi=None)})
        self.assertEqual(self.android.original(APP, PLAN, self.job)[1], "Google Play")
        self.assertEqual(len([c for c in self.calls if c[0].startswith("https://auroraoss.com")]), 2)

    def test_the_next_source_is_tried_when_one_fails_or_lies(self):
        bundle = self.files / "app.xapk"
        with zipfile.ZipFile(bundle, "w") as archive:
            archive.write(self.apk("base.apk"), "com.example.app.apk")
            archive.write(self.apk("split.apk", split="config.arm64_v8a", abi=None), "config.arm64_v8a.apk")
            archive.writestr("manifest.json", "{}")
        replies = self.play()
        info = message((5, "42"), (6, "1.2.3"), (24, message((8, "XAPK"), (9, "https://pure.example/app.xapk"))))
        older = message((5, "41"), (6, "1.2.2"), (24, message((8, "APK"), (9, "https://pure.example/old.apk"))))
        entries = [(2, message((3, message((2, version))))) for version in (older, info)]
        replies[android.PURE] = (200, message((1, message((7, message(*entries))))))
        # Play answers with a file somebody else signed.
        self.serve(replies, {"https://play.example/base": self.apk("forged.apk", key=STRANGER),
                             "https://play.example/split": self.apk("split2.apk", split="config.arm64_v8a", abi=None),
                             "https://pure.example/app.xapk": bundle})
        files, source = self.android.original(APP, PLAN, self.job)
        self.assertEqual((len(files), source), (2, "APKPure"))
        self.assertIn(b"Google Play: its file is not signed by Example Inc", self.builder.log.getvalue())
        self.assertEqual(list(self.job.iterdir()), [])

    def test_a_hand_placed_original_needs_no_download(self):
        originals = self.android.private / "originals"
        (originals / "notes.txt").write_text("not an app")
        package(originals / "forged.apk", key=STRANGER)
        package(originals / "newer.apk", code=43)
        bundle = originals / "example.apks"
        with zipfile.ZipFile(bundle, "w") as archive:
            archive.write(self.apk("base.apk"), "base.apk")
            archive.write(self.apk("split.apk", split="config.arm64_v8a", abi=None), "split_config.arm64_v8a.apk")
        files, source = self.android.original(APP, PLAN, self.job)
        self.assertEqual((len(files), source), (2, "the originals folder"))
        self.assertEqual(self.calls, [])
        self.assertTrue(bundle.is_file())

    def test_a_build_fails_plainly_when_no_source_has_the_original(self):
        self.serve({"https://auroraoss.com/api/auth": (429, b"")}, {})
        with self.assertRaises(BuildError) as raised:
            self.android.original(APP, PLAN, self.job)
        text = str(raised.exception)
        self.assertIn("Google Play: its anonymous sign-in is rate limited; try again later", text)
        self.assertIn("APKPure: it could not be reached", text)
        self.assertIn("automation/android/originals", text)
        self.assertFalse(raised.exception.blocked)

    def test_unreadable_replies_are_one_sources_failure(self):
        for reply in (b"\x0a\x7f", b"\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff\xff", b"\x03"):
            with self.assertRaises(android.SourceError):
                android.records(reply)
        self.assertEqual(android.nested(message((1, message((2, "a"), (2, "b"))), (3, 7)), 1, 2), [b"a", b"b"])
        self.assertIsNone(android.first(message((3, 7)), 3, 1))

    def test_plans_are_reproducible_and_builds_reject_a_changed_plan(self):
        app = self.android.app("android-youtube")

        def plan(options):
            out = io.StringIO()
            with redirect_stdout(out):
                self.android.plan(app, options)
            event = json.loads(out.getvalue())
            self.assertEqual(event["event"], "plan")
            return event["plan"]

        first = plan({})
        self.assertEqual((first["platform"], first["external_id"], first["version"]), ("android", str(app["version_code"]), app["app_version"]))
        self.assertEqual(first, plan({}))
        optional = [p["id"] for p in app["packages"] if p.get("optional")]
        if optional:
            self.assertNotEqual(first["key"], plan({"tweaks": optional[:1]})["key"])
        for options in ({"tweaks": ["unknown"]}, {"name": "Other"}):
            with self.assertRaises(BuildError):
                plan(options)
        changed = dict(first, version="0.0.1")
        with self.assertRaisesRegex(BuildError, "build plan changed"):
            self.android.build(app, changed, 1, self.job)
        with self.assertRaisesRegex(BuildError, "Invalid Kiwi build revision"):
            self.android.build(app, first, 0, self.job)
        with self.assertRaisesRegex(BuildError, "Unknown app recipe"):
            self.android.app("youtube")


class TransportTests(unittest.TestCase):
    def test_only_https_is_spoken(self):
        with tempfile.TemporaryDirectory() as d:
            with self.assertRaisesRegex(android.SourceError, "insecure"):
                android.http("http://example.com/")
            with self.assertRaisesRegex(android.SourceError, "insecure"):
                android.download("http://example.com/app.apk", Path(d) / "app.apk")
            with self.assertRaisesRegex(android.SourceError, "insecure"):
                android.download("file:///etc/passwd", Path(d) / "app.apk")
            self.assertEqual(list(Path(d).iterdir()), [])

    def test_bundles_are_unpacked_flat(self):
        with tempfile.TemporaryDirectory() as d:
            bundle = Path(d) / "app.apkm"
            with zipfile.ZipFile(bundle, "w") as archive:
                archive.writestr("../../escape.apk", b"one")
                archive.writestr("nested/base.APK", b"two")
                archive.writestr("icon.png", b"three")
            files = android.unpack_bundle(bundle, Path(d) / "out")
            self.assertEqual([f.name for f in files], ["000.apk", "001.apk"])
            self.assertEqual(sorted(p.name for p in Path(d).iterdir()), ["app.apkm", "out"])
            with zipfile.ZipFile(bundle, "w") as archive:
                archive.writestr("icon.png", b"three")
            with self.assertRaisesRegex(android.SourceError, "no usable packages"):
                android.unpack_bundle(bundle, Path(d) / "empty")
            bundle.write_bytes(b"not a zip")
            with self.assertRaisesRegex(android.SourceError, "unreadable"):
                android.unpack_bundle(bundle, Path(d) / "bad")


if __name__ == "__main__":
    unittest.main()
