import type { Builder, BuilderInfo, BuilderPairing, BuilderTask, Host, SetupCheck } from "../api/types";
import { formatRelative } from "./format";

/** What the owner runs on the Mac when its Kiwi is a different version than this one. */
export const UPDATE_COMMAND = "git pull && ./install.sh --builder";

/** How often Settings › Mac Builder asks whether the Mac has paired. */
export const PAIRING_POLL_MS = 3000;

/** The check kiwi-server lists first while a Mac builder is paired. */
export const BUILDER_CHECK = "Mac builder";
/** The check kiwi-server lists where iOS apps can't be built (`automation::NEEDS_MAC_CHECK`). */
export const NEEDS_MAC_CHECK = "macOS";

/** Settings › Mac Builder's state: what the screen shows. */
export type BuilderState = "unsupported" | "unpaired" | "pairing" | "paired";

/** A pending code wins over the paired Mac: pairing another Mac replaces it. */
export function builderState(info: BuilderInfo, now: number = Date.now()): BuilderState {
  if (!info.supported) return "unsupported";
  if (info.pairing && !expired(info.pairing, now)) return "pairing";
  return info.builder ? "paired" : "unpaired";
}

const expired = (pairing: Pick<BuilderPairing, "expires_at">, now: number) => {
  const at = Date.parse(pairing.expires_at);
  return Number.isNaN(at) || at <= now;
};

export interface Countdown {
  seconds: number;
  /** "9:41", "0:05", "0:00". */
  label: string;
  expired: boolean;
}

/** Time left on a pairing code, rounded up so it reads 0:00 only once the code has expired. */
export function countdown(expiresAt: string, now: number = Date.now()): Countdown {
  const at = Date.parse(expiresAt);
  const seconds = Number.isNaN(at) ? 0 : Math.max(0, Math.ceil((at - now) / 1000));
  const label = `${Math.floor(seconds / 60)}:${String(seconds % 60).padStart(2, "0")}`;
  return { seconds, label, expired: seconds === 0 };
}

/** The code this screen is waiting on, and the Mac (if any) that was paired when it was made. */
export interface PairingWatch {
  code: string;
  expires_at: string;
  /** `paired_at` of the builder paired before, so a replacement Mac counts as newly paired. */
  previous: string | null;
}

export function watchPairing(info: BuilderInfo): PairingWatch | null {
  return info.pairing
    ? { code: info.pairing.code, expires_at: info.pairing.expires_at, previous: info.builder?.paired_at ?? null }
    : null;
}

/**
 * What the latest `GET /api/builder` means for a code being waited on: keep polling, or stop
 * because a Mac paired, the code expired, or it was withdrawn (cancelled elsewhere).
 */
export type PairingStep = "poll" | "paired" | "expired" | "withdrawn";

export function pairingStep(watch: PairingWatch, info: BuilderInfo, now: number = Date.now()): PairingStep {
  if (info.builder && info.builder.paired_at !== watch.previous) return "paired";
  const pairing = info.supported ? info.pairing : null;
  if (!pairing) return expired(watch, now) ? "expired" : "withdrawn";
  // A newer code (made on another device) is waited on in its place.
  return expired(pairing, now) ? "expired" : "poll";
}

export type BuilderStatus = "online" | "offline" | "needs-update";

/** Offline first: an update can only be checked once the Mac connects again. */
export function builderStatus(builder: Pick<Builder, "online" | "compatible">): BuilderStatus {
  if (!builder.online) return "offline";
  return builder.compatible ? "online" : "needs-update";
}

export const STATUS_LABELS: Record<BuilderStatus, string> = {
  online: "Online",
  offline: "Offline",
  "needs-update": "Needs update",
};

/** "Online", "Offline · last seen 5 minutes ago" or "Needs update". */
export function statusText(builder: Builder, now: Date = new Date()): string {
  const status = builderStatus(builder);
  if (status !== "offline") return STATUS_LABELS[status];
  return builder.last_seen ? `Offline · last seen ${formatRelative(builder.last_seen, now)}` : "Offline · not seen yet";
}

/** Why iOS builds wait, for the paired Mac's card. */
export function statusDetail(builder: Builder): string {
  switch (builderStatus(builder)) {
    case "online":
      return builder.task ? taskLine(builder.task) : "Ready to build iOS apps.";
    case "offline":
      return `iOS builds wait until ${builder.name} is back. Check that it is awake and connected to Tailscale.`;
    case "needs-update":
      return `Kiwi on ${builder.name} is a different version. iOS builds wait until it is updated.`;
  }
}

/** "Building YouTube", "Checking YouTube" (for a new version) or "Checking the Mac" (its setup). */
export function taskTitle(task: BuilderTask): string {
  const what = task.name ?? task.app ?? "an app";
  if (task.command === "build") return `Building ${what}`;
  return task.command === "plan" ? `Checking ${what}` : "Checking the Mac";
}

/** "Building YouTube · Decrypting YouTube". */
export function taskLine(task: BuilderTask): string {
  const title = taskTitle(task);
  return task.stage && task.stage !== title ? `${title} · ${task.stage}` : title;
}

/** Settings' Mac Builder row: a short status. */
export function rowValue(info: BuilderInfo, now: number = Date.now()): { text: string; tone: "ok" | "warn" | null } {
  switch (builderState(info, now)) {
    case "unsupported":
    case "unpaired":
      return { text: "Not paired", tone: null };
    case "pairing":
      return { text: "Pairing…", tone: null };
    case "paired": {
      const status = builderStatus(info.builder!);
      return { text: info.builder!.task ? "Building" : STATUS_LABELS[status], tone: status === "online" ? "ok" : "warn" };
    }
  }
}

/** Where iOS apps are built: here (a Mac), on the paired Mac builder, nowhere yet, or not known yet. */
export type IosBuilds = "local" | "builder" | "none" | "unknown";

export function iosBuilds(host: Pick<Host, "os">, info: BuilderInfo | undefined): IosBuilds {
  if (host.os === "macos") return "local";
  if (!info) return "unknown";
  if (!info.supported) return "local";
  return info.builder ? "builder" : "none";
}

/** The fix for a setup check that isn't ready, as the Kiwi Builds pages show it. */
export const SETUP_HELP: Record<string, string> = {
  "Mac build tools": "Install Xcode and Kiwi's build tools on the Mac.",
  "Tweak toolchain": "Install the patch compiler using Kiwi's Mac setup instructions.",
  "App Store sign-in": "Sign in to Kiwi's App Store downloader on the Mac. Apple may ask for a verification code.",
  "Private decryption enrollment": "Install and open an iPhone or iPad app from the Mac App Store, then enroll the Mac.",
};

export interface CheckView {
  /** The row's label: "Mac Builder" for the builder's own checks. */
  label: string;
  ready: boolean;
  /** "Ready", "Setup needed", "Offline", "Needs update", "Not paired". */
  status: string;
  hint: string | null;
  /** A command to run on the Mac, on its own line. */
  command: string | null;
  /** The problem is fixed in Settings › Mac Builder. */
  settings: boolean;
}

/**
 * A setup check for the Kiwi Builds pages. The missing Mac (`macOS`) and the Mac builder's
 * own check point to Settings › Mac Builder, saying why (`info` from `GET /api/builder`, when known).
 */
export function checkView(check: SetupCheck, info?: BuilderInfo): CheckView {
  const view: CheckView = { label: check.name, ready: check.ready, status: "Ready", hint: null, command: null, settings: false };
  if (check.name === NEEDS_MAC_CHECK) {
    return { ...view, label: "Mac Builder", status: check.ready ? "Ready" : "Not paired", hint: check.ready ? null : "iOS apps need a Mac builder.", settings: !check.ready };
  }
  if (check.name === BUILDER_CHECK) {
    view.label = "Mac Builder";
    if (check.ready) return view;
    view.settings = true;
    const builder = info?.builder;
    if (!builder) return { ...view, status: info ? "Not paired" : "Not ready", hint: info ? "iOS apps need a Mac builder." : null };
    switch (builderStatus(builder)) {
      case "offline":
        return {
          ...view,
          status: "Offline",
          hint: `${builder.name} is offline${builder.last_seen ? ` (last seen ${formatRelative(builder.last_seen)})` : ""}. iOS builds wait until it is back.`,
        };
      case "needs-update":
        return { ...view, status: "Needs update", hint: `Kiwi on ${builder.name} is a different version. On the Mac:`, command: UPDATE_COMMAND };
      default:
        // Ready again since the checks were saved: the next look shows it.
        return { ...view, status: "Not ready", hint: null };
    }
  }
  return check.ready ? view : { ...view, status: "Setup needed", hint: SETUP_HELP[check.name] ?? null };
}
