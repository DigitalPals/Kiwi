import { describe, expect, it } from "vitest";
// The server's limits, to keep the browser's check in step.
import signerFiles from "../../../crates/kiwi-signer/src/files.rs?raw";
import { ApiError } from "../api/client";
import type { Identity, Profile, Signing } from "../api/types";
import {
  UPLOADS,
  bytesToBase64,
  checkUploadSize,
  filesIntro,
  readBase64,
  signingSummary,
  splitCommand,
  uploadErrorMessage,
} from "./signing";

const NOW = new Date("2026-11-10T12:00:00Z");

const identity: Identity = {
  sha1: "ABC",
  name: "Apple Development: Alex Appleseed (TEAM123456)",
  team_id: "TEAM123456",
  expires_at: "2027-09-04T00:00:00Z",
};

const profile: Profile = {
  uuid: "P-1",
  name: "Kiwi Wildcard",
  team_id: "TEAM123456",
  app_id_pattern: "TEAM123456.*",
  wildcard: true,
  development: true,
  created_at: "2026-09-04T00:00:00Z",
  expires_at: "2027-09-04T00:00:00Z",
  devices: 3,
  entitlements: [],
  usable: true,
  reason: null,
  uploaded: true,
};

function signing(patch: Partial<Signing> = {}): Signing {
  return { backend: "files", identities: [], profiles: [], active: null, error: null, ...patch };
}

describe("checkUploadSize", () => {
  it("accepts files up to the server's limits", () => {
    expect(checkUploadSize("identity", 3_000)).toBeNull();
    expect(checkUploadSize("identity", UPLOADS.identity.maxBytes)).toBeNull();
    expect(checkUploadSize("profile", UPLOADS.profile.maxBytes)).toBeNull();
  });

  it("refuses empty and oversized files before uploading", () => {
    expect(checkUploadSize("identity", 0)).toBe("The .p12 file is empty.");
    expect(checkUploadSize("identity", 64 * 1024 + 1)).toMatch(/^The \.p12 file is larger than 64 KB\./);
    expect(checkUploadSize("profile", 1024 * 1024 + 1)).toMatch(/^The provisioning profile is larger than 1 MB\./);
  });

  it("uses the server's limits", () => {
    const limit = (name: string) => {
      const expression = new RegExp(`const ${name}: usize = ([\\d_ *]+);`).exec(signerFiles)?.[1] ?? "0";
      return expression.split("*").reduce((product, factor) => product * Number(factor.replaceAll("_", "").trim()), 1);
    };
    expect(UPLOADS.identity.maxBytes).toBe(limit("MAX_P12_BYTES"));
    expect(UPLOADS.profile.maxBytes).toBe(limit("MAX_PROFILE_BYTES"));
  });
});

describe("base64", () => {
  it("encodes bytes as standard padded base64", () => {
    expect(bytesToBase64(new Uint8Array())).toBe("");
    expect(bytesToBase64(new Uint8Array([0x4b, 0x69, 0x77, 0x69]))).toBe("S2l3aQ==");
    expect(bytesToBase64(new Uint8Array([0xfb, 0xff, 0xbf]))).toBe("+/+/");
  });

  it("encodes files larger than one chunk", () => {
    const bytes = new Uint8Array(200_000).map((_, i) => (i * 31) % 256);
    const decoded = atob(bytesToBase64(bytes));
    expect(decoded).toHaveLength(bytes.length);
    expect([...decoded].every((c, i) => c.charCodeAt(0) === bytes[i])).toBe(true);
  });

  it("reads a chosen file", async () => {
    await expect(readBase64(new Blob([new Uint8Array([1, 2, 3, 250])]))).resolves.toBe("AQID+g==");
  });

  it("explains a file that can't be read", async () => {
    const broken = { arrayBuffer: () => Promise.reject(new Error("NotReadableError")) } as unknown as Blob;
    await expect(readBase64(broken)).rejects.toThrow("Kiwi couldn't read the file. Choose it again.");
  });
});

describe("uploadErrorMessage", () => {
  it("shows the server's message", () => {
    const wrong = new ApiError("The password is wrong", 400, "bad_request");
    expect(uploadErrorMessage("identity", wrong)).toBe("The password is wrong");
    expect(uploadErrorMessage("profile", new Error("Load failed"))).toBe("Load failed");
  });

  it("explains a refused body in terms of the file", () => {
    const large = new ApiError("Request body is too large.", 400, "bad_request");
    expect(uploadErrorMessage("identity", large)).toMatch(/^The \.p12 file is larger than 64 KB/);
    expect(uploadErrorMessage("profile", new ApiError("413 Payload Too Large", 413, "http"))).toMatch(
      /^The provisioning profile is larger than 1 MB/,
    );
  });
});

describe("splitCommand", () => {
  it("separates a command the owner can copy", () => {
    const message =
      "This .p12 uses AES encryption (the default of OpenSSL 3), which Kiwi can't open. Export it from Keychain Access, or convert it with: openssl pkcs12 -export -keypbe PBE-SHA1-3DES -certpbe PBE-SHA1-3DES -macalg sha1";
    expect(splitCommand(message)).toEqual({
      text: "This .p12 uses AES encryption (the default of OpenSSL 3), which Kiwi can't open. Export it from Keychain Access, or convert it with:",
      command: "openssl pkcs12 -export -keypbe PBE-SHA1-3DES -certpbe PBE-SHA1-3DES -macalg sha1",
    });
  });

  it("leaves other messages alone", () => {
    expect(splitCommand("The password is wrong")).toEqual({ text: "The password is wrong", command: null });
  });
});

describe("signingSummary", () => {
  it("says a Linux Kiwi with nothing uploaded isn't set up yet", () => {
    const summary = signingSummary(signing({ error: "No signing identity was uploaded. …" }), "linux", NOW);
    expect(summary).toEqual({
      tone: "warn",
      icon: "warning",
      title: "Signing Isn't Set Up",
      text: "This Kiwi runs on Linux, so it signs with a certificate and profiles you upload.",
    });
    expect(filesIntro("other")).toBe("This Kiwi doesn't run on a Mac, so it signs with a certificate and profiles you upload.");
    expect(filesIntro(undefined)).toBe("This Kiwi signs with a certificate and profiles you upload.");
  });

  it("names the missing upload while setting up", () => {
    const error = "No usable wildcard provisioning profile was found. Download a profile … and upload it.";
    expect(signingSummary(signing({ identities: [identity], error }), "linux", NOW)).toMatchObject({
      tone: "warn",
      title: "Signing Isn't Set Up",
      text: error,
    });
  });

  it("can't sign when the uploads don't fit together", () => {
    const error = "No usable wildcard provisioning profile was found.";
    const unusable = { ...profile, usable: false, reason: "Does not include the signing certificate" };
    expect(signingSummary(signing({ identities: [identity], profiles: [unusable], error }), "linux", NOW)).toMatchObject({
      tone: "error",
      title: "Can't Sign",
      text: error,
    });
    expect(signingSummary(signing({ backend: "keychain" }), "macos", NOW)).toMatchObject({ tone: "error", title: "Can't Sign" });
  });

  it("is ready with the identity and the expiry", () => {
    const active = { identity_sha1: "ABC", profile_uuid: "P-1", expires_at: "2027-09-04T00:00:00Z" };
    const summary = signingSummary(signing({ identities: [identity], profiles: [profile], active }), "linux", NOW);
    expect(summary.tone).toBe("ok");
    expect(summary.title).toBe("Ready to Sign");
    expect(summary.text).toMatch(/^New builds use Apple Development: Alex Appleseed \(TEAM123456\) and stay valid until .*2027\.$/);
  });

  it("says how to renew, per backend", () => {
    const active = { identity_sha1: "ABC", profile_uuid: "P-1", expires_at: "2026-11-20T00:00:00Z" };
    const files = signingSummary(signing({ identities: [identity], profiles: [profile], active }), "linux", NOW);
    expect(files).toMatchObject({ tone: "warn", icon: "clock", title: "Signing expires in 9 days" });
    expect(files.text).toMatch(/ Upload a renewed certificate or profile\.$/);
    const keychain = signingSummary(signing({ backend: "keychain", identities: [identity], profiles: [profile], active }), "macos", NOW);
    expect(keychain.text).toMatch(/ Renew the certificate or profile in Xcode, then rescan\.$/);
  });
});
