import { ApiError, errorMessage } from "../api/client";
import type { Host, Signing } from "../api/types";
import { expiryInfo, formatDate } from "./format";

/**
 * Signing files uploaded from the browser: the identity (a `.p12` with its password) and
 * provisioning profiles (`.mobileprovision`), sent to the API as base64.
 */
export type UploadKind = "identity" | "profile";

interface UploadSpec {
  /** The file input's `accept`. */
  accept: string;
  /** Largest file the server takes (kiwi-signer's `MAX_P12_BYTES` and `MAX_PROFILE_BYTES`). */
  maxBytes: number;
  /** The file in messages: "The .p12 file". */
  noun: string;
  /** What to do about a file that is too large. */
  tooLarge: string;
}

export const UPLOADS: Record<UploadKind, UploadSpec> = {
  identity: {
    accept: ".p12,.pfx",
    maxBytes: 64 * 1024,
    noun: "The .p12 file",
    tooLarge: "is larger than 64 KB. Export only the Apple Development certificate with its private key.",
  },
  profile: {
    // No extension filter: iOS greys out file types it doesn't know, and the server checks
    // every upload anyway.
    accept: "",
    maxBytes: 1024 * 1024,
    noun: "The provisioning profile",
    tooLarge: "is larger than 1 MB. Choose the .mobileprovision file from developer.apple.com.",
  },
};

/** Why a file of `size` bytes can't be uploaded as `kind`, or null when it can. */
export function checkUploadSize(kind: UploadKind, size: number): string | null {
  const spec = UPLOADS[kind];
  if (size <= 0) return `${spec.noun} is empty.`;
  if (size > spec.maxBytes) return `${spec.noun} ${spec.tooLarge}`;
  return null;
}

/** Standard base64 (with padding), as the API expects. */
export function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  // Spreading a whole file into fromCharCode would overflow the call stack.
  const chunk = 0x8000;
  for (let i = 0; i < bytes.length; i += chunk) {
    binary += String.fromCharCode(...bytes.subarray(i, i + chunk));
  }
  return btoa(binary);
}

/** Reads a chosen file as base64. */
export async function readBase64(file: Blob): Promise<string> {
  let buffer: ArrayBuffer;
  try {
    buffer = await file.arrayBuffer();
  } catch {
    throw new Error("Kiwi couldn't read the file. Choose it again.");
  }
  return bytesToBase64(new Uint8Array(buffer));
}

/** The message for a failed upload: the server's, or a size message when a proxy or the server refused the body. */
export function uploadErrorMessage(kind: UploadKind, error: unknown): string {
  const tooLarge =
    error instanceof ApiError &&
    (error.status === 413 || (error.status === 400 && error.message === "Request body is too large."));
  if (tooLarge) return `${UPLOADS[kind].noun} ${UPLOADS[kind].tooLarge}`;
  return errorMessage(error);
}

/**
 * Splits a message that ends in a shell command ("…, or convert it with: openssl pkcs12 …")
 * so the command can be shown on its own line, ready to copy.
 */
export function splitCommand(message: string): { text: string; command: string | null } {
  const match = /^(.*?:)\s+(openssl\s.+)$/s.exec(message);
  return match ? { text: match[1] ?? message, command: match[2] ?? null } : { text: message, command: null };
}

/** The first line of the status card on a Kiwi that signs with uploaded files. */
export function filesIntro(os: Host["os"] | undefined): string {
  const lead =
    os === "linux" ? "This Kiwi runs on Linux, so it signs" : os === "other" ? "This Kiwi doesn't run on a Mac, so it signs" : "This Kiwi signs";
  return `${lead} with a certificate and profiles you upload.`;
}

export interface SigningSummary {
  tone: "ok" | "warn" | "error";
  icon: "shield" | "clock" | "warning";
  title: string;
  text: string;
}

/** The answer first: can Kiwi sign, until when, and if not, what to do. */
export function signingSummary(data: Signing, os?: Host["os"], now: Date = new Date()): SigningSummary {
  const files = data.backend === "files";
  if (!data.active) {
    // Uploads still missing: setting up, not an error.
    if (files && (data.identities.length === 0 || data.profiles.length === 0)) {
      const nothing = data.identities.length === 0 && data.profiles.length === 0;
      return {
        tone: "warn",
        icon: "warning",
        title: "Signing Isn't Set Up",
        text: nothing ? filesIntro(os) : (data.error ?? filesIntro(os)),
      };
    }
    return {
      tone: "error",
      icon: "warning",
      title: "Can't Sign",
      text:
        data.error ??
        (files
          ? "No usable identity and wildcard profile were uploaded. The profile must include the uploaded certificate."
          : "No usable identity and wildcard profile were found. Open Xcode on the Mac, sign in to your team and let it create a development profile."),
    };
  }
  const identity = data.identities.find((i) => i.sha1 === data.active?.identity_sha1);
  const expiry = expiryInfo(data.active.expires_at, now);
  const soon = expiry !== null && expiry.tone !== "ok";
  let text = `New builds use ${identity?.name ?? "the active identity"} and stay valid until ${formatDate(data.active.expires_at)}.`;
  if (soon) text += files ? " Upload a renewed certificate or profile." : " Renew the certificate or profile in Xcode, then rescan.";
  return {
    tone: soon ? "warn" : "ok",
    icon: soon ? "clock" : "shield",
    title: soon ? `Signing ${expiry.label.toLowerCase()}` : "Ready to Sign",
    text,
  };
}
