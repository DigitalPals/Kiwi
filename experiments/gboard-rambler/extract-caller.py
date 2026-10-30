#!/usr/bin/env python3
"""Extract private service metadata from the exact APK inspected in the report.

No decompiler, Google account, personal API key or Android runtime is required.
Only the three header values are written, privately, to the requested output.
The APK and the generated configuration must stay outside source control.
"""
import argparse
import hashlib
import json
import os
from pathlib import Path
import re
import struct
import sys
import zipfile

APK_SHA256 = "7687e2cf5e72f6a6cab33adb94b60fd5af28b8656d30d64373e4f02c02d989c5"
HEADERS = ("X-Goog-Api-Key", "X-Android-Package", "X-Android-Cert")


class Dex:
    def __init__(self, data):
        if not data.startswith(b"dex\n") or len(data) < 112:
            raise ValueError("Invalid DEX header")
        self.data = data
        string_count, string_offset = self.pair(56)
        self.strings = []
        for index in range(string_count):
            offset = self.uint(string_offset + 4 * index)
            _, offset = self.uleb(offset)
            end = data.index(0, offset)
            self.strings.append(data[offset:end].decode("utf-8", "replace"))
        type_count, type_offset = self.pair(64)
        self.types = [self.strings[self.uint(type_offset + 4 * i)] for i in range(type_count)]

    def uint(self, offset):
        return struct.unpack_from("<I", self.data, offset)[0]

    def pair(self, offset):
        return struct.unpack_from("<II", self.data, offset)

    def uleb(self, offset):
        result = 0
        for shift in range(0, 35, 7):
            value = self.data[offset]
            offset += 1
            result |= (value & 127) << shift
            if value < 128:
                return result, offset
        raise ValueError("Invalid DEX integer")

    def connector_methods(self):
        count, offset = self.pair(96)
        for index in range(count):
            pos = offset + 32 * index
            if self.types[self.uint(pos)] != "Lilb;":
                continue
            pos = self.uint(pos + 24)
            if not pos:
                continue
            sizes = []
            for _ in range(4):
                value, pos = self.uleb(pos)
                sizes.append(value)
            for _ in range(sizes[0] + sizes[1]):
                _, pos = self.uleb(pos)
                _, pos = self.uleb(pos)
            for _ in range(sizes[2] + sizes[3]):
                _, pos = self.uleb(pos)
                _, pos = self.uleb(pos)
                code, pos = self.uleb(pos)
                if code:
                    yield code

    def caller(self):
        for code in self.connector_methods():
            count = self.uint(code + 12)
            words = struct.unpack_from(f"<{count}H", self.data, code + 16)
            strings = []
            # In this hash-pinned connector the header and value are adjacent
            # const-string references. Scanning code units is deliberately
            # restricted to this class and validated against all three formats.
            for i in range(len(words) - 1):
                opcode = words[i] & 255
                if opcode == 0x1a:
                    index = words[i + 1]
                elif opcode == 0x1b and i + 2 < len(words):
                    index = words[i + 1] | words[i + 2] << 16
                else:
                    continue
                if index < len(self.strings):
                    strings.append(self.strings[index])
            found = {}
            for header in HEADERS:
                candidates = [strings[i + 1] for i, value in enumerate(strings[:-1]) if value == header]
                if len(candidates) == 1:
                    found[header] = candidates[0]
            if len(found) == 3:
                if not re.fullmatch(r"AIza[\w-]{35}", found[HEADERS[0]]):
                    raise ValueError("Unexpected app key format")
                if not re.fullmatch(r"[A-Za-z][A-Za-z0-9_.]+", found[HEADERS[1]]):
                    raise ValueError("Unexpected Android package format")
                if not re.fullmatch(r"[a-fA-F0-9]{40}", found[HEADERS[2]]):
                    raise ValueError("Unexpected Android certificate digest format")
                return found
        return None


def extract(apk):
    with apk.open("rb") as source:
        sha256 = hashlib.file_digest(source, "sha256").hexdigest()
    if sha256 != APK_SHA256:
        raise ValueError("APK hash differs from the inspected PixelBoard 1.3.3 artifact")
    with zipfile.ZipFile(apk) as archive:
        for entry in archive.infolist():
            if re.fullmatch(r"classes\d*\.dex", entry.filename):
                if entry.file_size > 100 * 1024 * 1024:
                    raise ValueError("DEX exceeds its limit")
                found = Dex(archive.read(entry)).caller()
                if found:
                    return {"headers": found}
    raise ValueError("No complete S3 caller configuration found")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("apk", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    try:
        configuration = extract(args.apk)
        # Refuse to overwrite a path, including symlinks. Credentials never go to
        # stdout, logs or exceptions; no destination is implicitly chosen.
        fd = os.open(args.output, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
        with os.fdopen(fd, "w") as output:
            json.dump(configuration, output)
            output.write("\n")
    except (OSError, ValueError, IndexError, struct.error, zipfile.BadZipFile):
        print("Extraction failed. Check the pinned APK and use a new private output path.", file=sys.stderr)
        return 1
    print("Private caller configuration extracted. Values omitted.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
