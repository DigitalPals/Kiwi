#!/usr/bin/env python3
"""Reads Android packages and proves who signed them, with the standard library only.

Kiwi downloads original Android apps from places it does not control. Before one
is patched, this module checks the APK Signature Scheme v2/v3 signature over the
whole file and reports the signing certificates, so the caller can compare them
with the publisher's certificate pinned in the recipe. A file that passes is
byte-for-byte what the publisher signed, wherever it came from.
"""

from __future__ import annotations

import hashlib
import json
import struct
import sys
from pathlib import Path

MAGIC = b"APK Sig Block 42"
SCHEMES = {0x7109871A: 2, 0xF05368C0: 3}
CHUNK = 1 << 20
LIMIT = 4 << 30

# Signature algorithm id -> (kind, hash). The verity variants are not supported.
ALGORITHMS = {
    0x0101: ("pss", "sha256"),
    0x0102: ("pss", "sha512"),
    0x0103: ("pkcs1", "sha256"),
    0x0104: ("pkcs1", "sha512"),
    0x0201: ("ecdsa", "sha256"),
    0x0202: ("ecdsa", "sha512"),
}
DIGEST_INFO = {
    "sha256": bytes.fromhex("3031300d060960864801650304020105000420"),
    "sha512": bytes.fromhex("3051300d060960864801650304020305000440"),
}
RSA = bytes.fromhex("2a864886f70d010101")
EC = bytes.fromhex("2a8648ce3d0201")
# Curve OID -> (p, a, b, gx, gy, n)
CURVES = {
    bytes.fromhex("2a8648ce3d030107"): (
        0xFFFFFFFF00000001000000000000000000000000FFFFFFFFFFFFFFFFFFFFFFFF,
        -3,
        0x5AC635D8AA3A93E7B3EBBD55769886BC651D06B0CC53B0F63BCE3C3E27D2604B,
        0x6B17D1F2E12C4247F8BCE6E563A440F277037D812DEB33A0F4A13945D898C296,
        0x4FE342E2FE1A7F9B8EE7EB4A7C0F9E162BCE33576B315ECECBB6406837BF51F5,
        0xFFFFFFFF00000000FFFFFFFFFFFFFFFFBCE6FAADA7179E84F3B9CAC2FC632551,
    ),
    bytes.fromhex("2b81040022"): (
        0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFEFFFFFFFF0000000000000000FFFFFFFF,
        -3,
        0xB3312FA7E23EE7E4988E056BE3F82D19181D9C6EFE8141120314088F5013875AC656398D8A2ED19D2A85C8EDD3EC2AEF,
        0xAA87CA22BE8B05378EB1C71EF320AD746E1D3B628BA79B9859F741E082542A385502F25DBF55296C3A545E3872760AB7,
        0x3617DE4A96262C6F5D9E98BF9292DC29F8F41DBD289A147CE9DA3113B5F0B8C00A60B1CE1D7E819D7A431D7C90EA0E5F,
        0xFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFC7634D81F4372DDF581A0DB248B0A77AECEC196ACCC52973,
    ),
}

# Manifest attributes are matched by resource id: their names can be stripped.
ATTRIBUTES = {
    0x0101021B: "version_code",
    0x0101021C: "version",
    0x0101020C: "min_sdk",
    0x01010270: "target_sdk",
}


class ApkError(Exception):
    """The file is not an Android package Kiwi can trust."""


class Reader:
    """A cursor over the length-prefixed records of a signing block."""

    def __init__(self, data: bytes):
        self.data = data
        self.offset = 0

    def take(self, size: int) -> bytes:
        end = self.offset + size
        if size < 0 or end > len(self.data):
            raise ApkError("the signature is truncated")
        value = self.data[self.offset : end]
        self.offset = end
        return value

    def u32(self) -> int:
        return struct.unpack("<I", self.take(4))[0]

    def field(self) -> bytes:
        return self.take(self.u32())

    def fields(self) -> list[bytes]:
        sequence = Reader(self.field())
        values = []
        while sequence.offset < len(sequence.data):
            values.append(sequence.field())
        return values


# --- binary manifest ---------------------------------------------------------


def _strings(chunk: bytes) -> list[str]:
    count, _, flags, start = struct.unpack_from("<IIII", chunk, 8)
    header = struct.unpack_from("<H", chunk, 2)[0]
    utf8 = bool(flags & 0x100)
    if header + 4 * count > len(chunk):
        raise ApkError("the manifest string table is truncated")
    values = []
    for index in range(count):
        offset = start + struct.unpack_from("<I", chunk, header + 4 * index)[0]
        try:
            if utf8:
                for _ in range(2):  # characters, then bytes
                    size = chunk[offset]
                    offset += 1
                    if size & 0x80:
                        size = (size & 0x7F) << 8 | chunk[offset]
                        offset += 1
                values.append(chunk[offset : offset + size].decode("utf-8", "replace"))
            else:
                size = struct.unpack_from("<H", chunk, offset)[0]
                offset += 2
                if size & 0x8000:
                    size = (size & 0x7FFF) << 16 | struct.unpack_from("<H", chunk, offset)[0]
                    offset += 2
                values.append(chunk[offset : offset + 2 * size].decode("utf-16-le", "replace"))
        except (IndexError, struct.error) as error:
            raise ApkError("the manifest string table is truncated") from error
    return values


def manifest(data: bytes) -> dict:
    """Package, version and SDK levels from a compiled AndroidManifest.xml."""
    if len(data) < 8 or struct.unpack_from("<H", data)[0] != 0x0003:
        raise ApkError("the manifest is not compiled Android XML")
    strings: list[str] = []
    resources: list[int] = []
    found: dict = {}
    depth = 0
    offset = struct.unpack_from("<H", data, 2)[0]
    while offset + 8 <= len(data):
        kind, header, size = struct.unpack_from("<HHI", data, offset)
        if size < 8 or offset + size > len(data):
            raise ApkError("the manifest is truncated")
        chunk = data[offset : offset + size]
        offset += size
        if kind == 0x0001 and not strings:
            strings = _strings(chunk)
        elif kind == 0x0180:
            resources = list(struct.unpack_from(f"<{(size - header) // 4}I", chunk, header))
        elif kind == 0x0103:
            depth -= 1
        elif kind == 0x0102:
            depth += 1
            name, start, width, count = struct.unpack_from("<IHHH", chunk, header + 4)
            element = strings[name] if name < len(strings) else ""
            wanted = (depth == 1 and element == "manifest") or (
                depth == 2 and element == "uses-sdk"
            )
            if not wanted or width < 20:
                continue
            for index in range(count):
                at = header + start + index * width
                if at + 20 > len(chunk):
                    raise ApkError("the manifest is truncated")
                _, key, raw, _, _, kind_, value = struct.unpack_from("<IIIHBBI", chunk, at)
                label = ATTRIBUTES.get(resources[key]) if key < len(resources) else None
                if label is None and key < len(strings) and strings[key] in ("package", "split"):
                    if key >= len(resources) or resources[key] == 0:
                        label = strings[key]
                if label is None or label in found:
                    continue
                if kind_ == 0x03:
                    found[label] = strings[value] if value < len(strings) else ""
                elif kind_ in (0x10, 0x11):
                    found[label] = value
                elif raw < len(strings):
                    found[label] = strings[raw]
    if not isinstance(found.get("package"), str) or not found["package"]:
        raise ApkError("the manifest names no package")
    if not isinstance(found.get("version_code"), int):
        raise ApkError("the manifest has no version code")
    found["version"] = str(found.get("version", ""))
    for key in ("min_sdk", "target_sdk"):
        if not isinstance(found.get(key), int):
            found.pop(key, None)
    return found


# --- signature ---------------------------------------------------------------


def _der(data: bytes, offset: int = 0) -> tuple[int, int, int]:
    """Tag, value start and value end of the DER element at `offset`."""
    try:
        tag = data[offset]
        size = data[offset + 1]
        start = offset + 2
        if size & 0x80:
            width = size & 0x7F
            if not 0 < width <= 4:
                raise ApkError("unsupported certificate encoding")
            size = int.from_bytes(data[start : start + width], "big")
            start += width
    except IndexError as error:
        raise ApkError("a certificate is truncated") from error
    if start + size > len(data):
        raise ApkError("a certificate is truncated")
    return tag, start, start + size


def _children(data: bytes, start: int, end: int) -> list[tuple[int, int, int, int]]:
    """(tag, element start, value start, value end) of each element in a range."""
    items = []
    while start < end:
        tag, value, stop = _der(data, start)
        items.append((tag, start, value, stop))
        start = stop
    return items


def public_key_of(certificate: bytes) -> bytes:
    """The SubjectPublicKeyInfo of an X.509 certificate, as encoded."""
    tag, start, end = _der(certificate)
    if tag != 0x30:
        raise ApkError("a signing certificate is not X.509")
    tbs = _children(certificate, start, end)[0]
    items = _children(certificate, tbs[2], tbs[3])
    if items and items[0][0] == 0xA0:
        items = items[1:]
    if len(items) < 6 or items[5][0] != 0x30:
        raise ApkError("a signing certificate is not X.509")
    return certificate[items[5][1] : items[5][3]]


def _key(spki: bytes):
    tag, start, end = _der(spki)
    parts = _children(spki, start, end)
    if tag != 0x30 or len(parts) != 2 or parts[1][0] != 0x03:
        raise ApkError("unsupported signing key")
    algorithm = _children(spki, parts[0][2], parts[0][3])
    oid = spki[algorithm[0][2] : algorithm[0][3]]
    bits = spki[parts[1][2] + 1 : parts[1][3]]
    if oid == RSA:
        _, start, end = _der(bits)
        numbers = _children(bits, start, end)
        modulus = int.from_bytes(bits[numbers[0][2] : numbers[0][3]], "big")
        exponent = int.from_bytes(bits[numbers[1][2] : numbers[1][3]], "big")
        if modulus.bit_length() < 1024:
            raise ApkError("the signing key is too short")
        return "rsa", (modulus, exponent)
    if oid == EC and len(algorithm) == 2:
        curve = CURVES.get(spki[algorithm[1][2] : algorithm[1][3]])
        if curve is None or not bits or bits[0] != 4:
            raise ApkError("unsupported signing key")
        width = (len(bits) - 1) // 2
        point = (
            int.from_bytes(bits[1 : 1 + width], "big"),
            int.from_bytes(bits[1 + width :], "big"),
        )
        p, a, b = curve[:3]
        if (point[1] ** 2 - point[0] ** 3 - a * point[0] - b) % p:
            raise ApkError("the signing key is not on its curve")
        return "ec", (curve, point)
    raise ApkError("unsupported signing key")


def _mgf1(seed: bytes, size: int, name: str) -> bytes:
    out = b""
    counter = 0
    while len(out) < size:
        out += hashlib.new(name, seed + counter.to_bytes(4, "big")).digest()
        counter += 1
    return out[:size]


def _rsa(key, kind: str, name: str, message: bytes, signature: bytes) -> bool:
    modulus, exponent = key
    width = (modulus.bit_length() + 7) // 8
    number = int.from_bytes(signature, "big")
    if len(signature) != width or number >= modulus:
        return False
    digest = hashlib.new(name, message).digest()
    encoded = pow(number, exponent, modulus)
    if kind == "pkcs1":
        tail = DIGEST_INFO[name] + digest
        padding = width - len(tail) - 3
        return padding >= 8 and encoded.to_bytes(width, "big") == (
            b"\x00\x01" + b"\xff" * padding + b"\x00" + tail
        )
    bits = modulus.bit_length() - 1
    size = (bits + 7) // 8
    if encoded.bit_length() > 8 * size:
        return False
    block = encoded.to_bytes(size, "big")
    salt = len(digest)
    if size < 2 * salt + 2 or block[-1] != 0xBC:
        return False
    masked, seed = block[: size - salt - 1], block[size - salt - 1 : -1]
    spare = 8 * size - bits
    if masked[0] >> (8 - spare) if spare else 0:
        return False
    plain = bytearray(x ^ y for x, y in zip(masked, _mgf1(seed, len(masked), name)))
    if spare:
        plain[0] &= 0xFF >> spare
    if any(plain[: -salt - 1]) or plain[-salt - 1] != 1:
        return False
    return hashlib.new(name, bytes(8) + digest + bytes(plain[-salt:])).digest() == seed


def _add(curve, left, right):
    p, a = curve[0], curve[1]
    if left is None:
        return right
    if right is None:
        return left
    if left[0] == right[0]:
        if (left[1] + right[1]) % p == 0:
            return None
        slope = (3 * left[0] * left[0] + a) * pow(2 * left[1], -1, p) % p
    else:
        slope = (right[1] - left[1]) * pow(right[0] - left[0], -1, p) % p
    x = (slope * slope - left[0] - right[0]) % p
    return x, (slope * (left[0] - x) - left[1]) % p


def _multiply(curve, scalar: int, point):
    result = None
    while scalar:
        if scalar & 1:
            result = _add(curve, result, point)
        point = _add(curve, point, point)
        scalar >>= 1
    return result


def _ecdsa(key, name: str, message: bytes, signature: bytes) -> bool:
    curve, point = key
    order = curve[5]
    try:
        tag, start, end = _der(signature)
        numbers = _children(signature, start, end)
    except ApkError:
        return False
    if tag != 0x30 or len(numbers) != 2:
        return False
    r = int.from_bytes(signature[numbers[0][2] : numbers[0][3]], "big")
    s = int.from_bytes(signature[numbers[1][2] : numbers[1][3]], "big")
    if not (0 < r < order and 0 < s < order):
        return False
    digest = hashlib.new(name, message).digest()
    value = int.from_bytes(digest, "big")
    extra = 8 * len(digest) - order.bit_length()
    if extra > 0:
        value >>= extra
    inverse = pow(s, -1, order)
    total = _add(
        curve,
        _multiply(curve, value * inverse % order, (curve[3], curve[4])),
        _multiply(curve, r * inverse % order, point),
    )
    return total is not None and total[0] % order == r


def _signed(spki: bytes, algorithm: int, message: bytes, signature: bytes) -> bool:
    kind, name = ALGORITHMS[algorithm]
    family, key = _key(spki)
    if family == "rsa" and kind in ("pss", "pkcs1"):
        return _rsa(key, kind, name, message, signature)
    if family == "ec" and kind == "ecdsa":
        return _ecdsa(key, name, message, signature)
    return False


def _layout(handle, size: int) -> tuple[int, int, int, bytes, dict[int, bytes]]:
    """Signing block offset, central directory offset and size, EOCD, block values."""
    tail = min(size, 65557)
    handle.seek(size - tail)
    data = handle.read(tail)
    at = len(data) - 22
    while at >= 0:
        if data[at : at + 4] == b"PK\x05\x06" and at + 22 + struct.unpack_from(
            "<H", data, at + 20
        )[0] == len(data):
            break
        at -= 1
    if at < 0:
        raise ApkError("the file is not a ZIP archive")
    eocd = data[at:]
    eocd_offset = size - len(eocd)
    directory_size, directory = struct.unpack_from("<II", eocd, 12)
    if directory == 0xFFFFFFFF or directory + directory_size != eocd_offset or directory < 32:
        raise ApkError("the archive layout is not supported")
    handle.seek(directory - 24)
    footer = handle.read(24)
    if footer[8:] != MAGIC:
        raise ApkError("the package has no APK signature (v2 or v3)")
    length = struct.unpack("<Q", footer[:8])[0]
    if length < 24 or length > 64 << 20 or length + 8 > directory:
        raise ApkError("the signature block is malformed")
    block = directory - length - 8
    handle.seek(block)
    body = handle.read(length + 8)
    if struct.unpack_from("<Q", body)[0] != length:
        raise ApkError("the signature block is malformed")
    values: dict[int, bytes] = {}
    at, end = 8, len(body) - 24
    while at < end:
        width = struct.unpack_from("<Q", body, at)[0]
        if width < 4 or at + 8 + width > end:
            raise ApkError("the signature block is malformed")
        values[struct.unpack_from("<I", body, at + 8)[0]] = body[at + 12 : at + 8 + width]
        at += 8 + width
    return block, directory, directory_size, eocd, values


def _content_digest(handle, block: int, directory: int, directory_size: int, eocd: bytes, name: str) -> bytes:
    """The chunked digest APK Signature Scheme v2 signs."""
    patched = eocd[:16] + struct.pack("<I", block) + eocd[20:]
    digests = []

    def chunks(data: bytes) -> None:
        for at in range(0, len(data), CHUNK):
            part = data[at : at + CHUNK]
            digests.append(
                hashlib.new(name, b"\xa5" + struct.pack("<I", len(part)) + part).digest()
            )

    for start, length in ((0, block), (directory, directory_size)):
        handle.seek(start)
        while length:
            part = handle.read(min(CHUNK, length))
            if not part:
                raise ApkError("the file is truncated")
            length -= len(part)
            chunks(part)
    chunks(patched)
    return hashlib.new(
        name, b"\x5a" + struct.pack("<I", len(digests)) + b"".join(digests)
    ).digest()


def _signer(data: bytes, scheme: int, digest) -> bytes | None:
    """Verifies one signer and returns its certificate, or None if unsupported."""
    reader = Reader(data)
    signed = reader.field()
    if scheme == 3:
        reader.take(8)
    signatures = []
    for item in reader.fields():
        record = Reader(item)
        signatures.append((record.u32(), record.field()))
    spki = reader.field()
    supported = [entry for entry in signatures if entry[0] in ALGORITHMS]
    if not supported:
        return None
    # Android trusts the strongest signature only; so does Kiwi.
    algorithm, signature = max(supported, key=lambda entry: ALGORITHMS[entry[0]][1])
    if not _signed(spki, algorithm, signed, signature):
        raise ApkError("the package signature does not verify")
    body = Reader(signed)
    digests = {}
    for item in body.fields():
        record = Reader(item)
        algorithm_ = record.u32()
        digests[algorithm_] = record.field()
    certificates = body.fields()
    if not certificates or public_key_of(certificates[0]) != spki:
        raise ApkError("the signing key does not match its certificate")
    name = ALGORITHMS[algorithm][1]
    if digests.get(algorithm) != digest(name):
        raise ApkError("the package was changed after it was signed")
    return certificates[0]


def inspect(path: Path) -> dict:
    """Verifies an APK's signature and returns its manifest and signers.

    `certificates` holds the SHA-256 of every signing certificate that was
    verified; the caller decides which publishers it trusts.
    """
    import zipfile

    path = Path(path)
    size = path.stat().st_size
    if not 0 < size < LIMIT:
        raise ApkError("the file is not a usable Android package")
    with path.open("rb") as handle:
        block, directory, directory_size, eocd, values = _layout(handle, size)
        cache: dict[str, bytes] = {}

        def digest(name: str) -> bytes:
            if name not in cache:
                cache[name] = _content_digest(handle, block, directory, directory_size, eocd, name)
            return cache[name]

        certificates: list[str] = []
        schemes: list[int] = []
        for identifier, scheme in sorted(SCHEMES.items(), key=lambda entry: entry[1]):
            if identifier not in values:
                continue
            signers = Reader(values[identifier]).fields()
            if not signers:
                raise ApkError("the signature block is malformed")
            verified = [_signer(signer, scheme, digest) for signer in signers]
            if scheme == 2 and None in verified:
                raise ApkError("the package uses an unsupported signature algorithm")
            verified = [item for item in verified if item is not None]
            if not verified:
                raise ApkError("the package uses an unsupported signature algorithm")
            schemes.append(scheme)
            for certificate in verified:
                fingerprint = hashlib.sha256(certificate).hexdigest()
                if fingerprint not in certificates:
                    certificates.append(fingerprint)
        if not schemes:
            raise ApkError("the package has no APK signature (v2 or v3)")
    try:
        with zipfile.ZipFile(path) as archive:
            info = archive.getinfo("AndroidManifest.xml")
            if info.file_size > 16 << 20:
                raise ApkError("the manifest is too large")
            details = manifest(archive.read(info))
            libraries = sorted(
                {
                    name.split("/")[1]
                    for name in archive.namelist()
                    if name.startswith("lib/") and name.count("/") == 2
                }
            )
    except (KeyError, zipfile.BadZipFile) as error:
        raise ApkError("the file is not an Android package") from error
    details.update(certificates=certificates, schemes=schemes, abis=libraries, size=size)
    return details


def main() -> int:
    status = 0
    for name in sys.argv[1:]:
        try:
            print(json.dumps({"file": name, **inspect(Path(name))}, sort_keys=True))
        except (ApkError, OSError) as error:
            print(json.dumps({"file": name, "error": str(error)}))
            status = 1
    return status


if __name__ == "__main__":
    raise SystemExit(main())
