#!/usr/bin/env python3
"""Package the native client into the inspected Gboard app and its keyboard.

No proprietary binaries or caller values are stored in this source. Unknown
Gboard images, encrypted code and insufficient Mach-O header padding fail closed.
"""
import hashlib
import os
from pathlib import Path, PurePosixPath
import plistlib
import stat
import struct
import sys
import tempfile
import zipfile

IMAGES = {
    "Gboard": "4982f84e3c89687fc61c69339fd79702de6da077bc924d09c02f0188c42a17b3",
    "Frameworks/gboard_framework.framework/gboard_framework": "387b02950243520eac35a6e4227b4515be22cb3b805e432dffd800ac9db56496",
    "PlugIns/Gboard.appex/Gboard": "711bc3dbeaec7cce6551d48286a7e5322b460e78aaaafa580e2ccd7fafb86566",
}
GROUPS = {
    "group.com.google.keyboard": "group.com.kiwi.gboard.{team}.keyboard",
    "group.com.google.common": "group.com.kiwi.gboard.{team}.common",
}


def inject(data, library):
    data = bytearray(data)
    if len(data) < 32 or struct.unpack_from("<II", data) != (0xfeedfacf, 0x100000c):
        raise ValueError("Expected the inspected thin arm64 Mach-O")
    count, size = struct.unpack_from("<II", data, 16)
    end = 32 + size
    if count > 4096 or end > len(data):
        raise ValueError("Invalid Mach-O load commands")
    position = 32
    first_section = len(data)
    for _ in range(count):
        if position + 8 > end:
            raise ValueError("Truncated Mach-O load command")
        command, length = struct.unpack_from("<II", data, position)
        if length < 8 or position + length > end:
            raise ValueError("Invalid Mach-O load command length")
        if command == 0x2c and (length < 24 or struct.unpack_from("<I", data, position + 16)[0]):
            raise ValueError("Gboard code remains encrypted")
        if command == 0x19:
            if length < 72:
                raise ValueError("Truncated Mach-O segment")
            sections = struct.unpack_from("<I", data, position + 64)[0]
            if 72 + sections * 80 > length:
                raise ValueError("Truncated Mach-O sections")
            for index in range(sections):
                section = position + 72 + 80 * index
                amount = struct.unpack_from("<Q", data, section + 40)[0]
                offset = struct.unpack_from("<I", data, section + 48)[0]
                flags = struct.unpack_from("<I", data, section + 64)[0] & 0xff
                if amount and offset and flags not in (1, 12, 18):
                    first_section = min(first_section, offset)
        position += length
    if position != end:
        raise ValueError("Inconsistent Mach-O command size")
    name = library.encode() + b"\0"
    length = (24 + len(name) + 7) // 8 * 8
    command = (struct.pack("<IIIIII", 0xc, length, 24, 0, 0, 0) + name).ljust(length, b"\0")
    if end + length > first_section or end + length > len(data) or any(data[end:end + length]):
        raise ValueError("Gboard has insufficient unused Mach-O header padding")
    data[end:end + length] = command
    struct.pack_into("<II", data, 16, count + 1, size + length)
    return bytes(data)


def _package(source, library, caller, sample, target):
    with zipfile.ZipFile(source) as original:
        entries = original.infolist()
        roots = [e.filename.rsplit("/", 1)[0] for e in entries
                 if len(PurePosixPath(e.filename).parts) == 3
                 and e.filename.startswith("Payload/") and e.filename.endswith(".app/Info.plist")]
        if len(roots) != 1:
            raise ValueError("Expected exactly one Gboard app")
        root = roots[0]
        info = plistlib.loads(original.read(root + "/Info.plist"))
        if info.get("CFBundleIdentifier") != "com.google.keyboard" or info.get("CFBundleVersion") != "2.3.19.444584732":
            raise ValueError("This integration requires Gboard 2.3.19 (2.3.19.444584732)")
        for image, sha in IMAGES.items():
            if hashlib.sha256(original.read(root + "/" + image)).hexdigest() != sha:
                raise ValueError("The Gboard images differ from the inspected build")
        seen = set()
        with zipfile.ZipFile(target, "w", zipfile.ZIP_DEFLATED) as output:
            for entry in entries:
                path = PurePosixPath(entry.filename)
                if path.is_absolute() or ".." in path.parts or "\\" in entry.filename or entry.filename in seen:
                    raise ValueError("Unsafe or duplicate IPA entry")
                seen.add(entry.filename)
                if stat.S_ISLNK(entry.external_attr >> 16):
                    raise ValueError("Unexpected symlink in the inspected Gboard IPA")
                if any(p.startswith("._") for p in path.parts):
                    continue
                relative = entry.filename.removeprefix(root + "/")
                if relative in ("Gboard", "PlugIns/Gboard.appex/Gboard"):
                    load = "@executable_path/" + ("../../" if relative.startswith("PlugIns/") else "") + "Frameworks/KiwiGboardRambler.dylib"
                    data = inject(original.read(entry), load)
                elif relative in ("Info.plist", "PlugIns/Gboard.appex/Info.plist"):
                    value = plistlib.loads(original.read(entry))
                    value["MinimumOSVersion"] = "16.0"
                    value["CFBundleDisplayName"] = "Gboard Rambler"
                    value["KiwiRequiredAppGroups"] = GROUPS
                    value["KiwiRedirectContainingAppURLs"] = True
                    # The signer adds the app's unique Kiwi URL scheme. The
                    # native bridge translates it back for Gboard's URL router.
                    for url in value.get("CFBundleURLTypes", []):
                        url["CFBundleURLSchemes"] = [s for s in url.get("CFBundleURLSchemes", []) if s != "gboard"]
                    value["CFBundleURLTypes"] = [u for u in value.get("CFBundleURLTypes", []) if u["CFBundleURLSchemes"]]
                    data = plistlib.dumps(value)
                else:
                    with original.open(entry) as src, output.open(entry, "w") as dst:
                        import shutil
                        shutil.copyfileobj(src, dst, 1024 * 1024)
                    continue
                output.writestr(entry, data)
            for name, path, mode in [
                ("Frameworks/KiwiGboardRambler.dylib", library, 0o100755),
                ("KiwiRamblerCaller.json", caller, 0o100644),
                ("KiwiRamblerSample.wav", sample, 0o100644),
            ]:
                entry = zipfile.ZipInfo(root + "/" + name)
                if entry.filename in seen:
                    raise ValueError("The Gboard input is already modified")
                entry.external_attr = mode << 16
                entry.compress_type = zipfile.ZIP_DEFLATED
                output.writestr(entry, Path(path).read_bytes())


def package(source, library, caller, sample, target):
    # Publish only a complete archive, without replacing an existing output or
    # deleting an unrelated file when validation fails (even if output == input).
    target = Path(target)
    with tempfile.NamedTemporaryFile(dir=target.parent, prefix=".gboard-", suffix=".ipa") as staged:
        _package(source, library, caller, sample, staged)
        staged.flush()
        os.link(staged.name, target)


if __name__ == "__main__":
    if len(sys.argv) != 7:
        raise SystemExit("Usage: inject-gboard.py INPUT.ipa CLIENT.dylib CALLER.json SAMPLE.wav OUTPUT.ipa --pinned")
    if sys.argv[6] != "--pinned":
        raise SystemExit("Only the inspected Gboard build is supported")
    try:
        package(*(Path(p) for p in sys.argv[1:6]))
    except Exception as error:
        # Neither a caller value nor contents of arbitrary input files are logged.
        raise SystemExit("Gboard packaging failed: " + str(error)) from None
